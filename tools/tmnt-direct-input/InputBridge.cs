using System;
using System.IO;
using System.Net.Sockets;
using System.Reflection;
using System.Threading;

namespace WALDirect
{
    // Called from IOManager.Update before UpdateCombinedInputs. This deliberately has no
    // BepInEx or Unity references so it can run as a normal managed sidecar assembly.
    public static class InputBridge
    {
        private static volatile byte[] frame = new byte[64];
        private static int started;
        private static FieldInfo inputDataField;
        private static FieldInfo switchCountField;
        private static FieldInfo switchCountLastField;
        private static FieldInfo inputTimeField;
        private static PropertyInfo deltaTimeProperty;
        private static uint volumeUpMask = uint.MaxValue;
        private static uint volumeDownMask;
        private static readonly bool[] switchStates = new bool[50];
        private static readonly uint[] switchCounts = new uint[50];
        private static bool switchStateInitialized;
        private static readonly int[] combinedRoots = { 4, 10, 15, 20, 25, 30, 35, 40, 45 };

        // Pawn.UpdateJump expects true only when the pawn has landed. The bundled
        // plugin delegates this calculation to its host process; without that host
        // it leaves height/velocity unchanged and returns false.
        public static bool GetNewHeight(ref float baseOffset, ref float velocity,
                                        float deltaTime, bool gravityOn, float gravity)
        {
            if (deltaTime <= 0f)
                return false;
            if (gravityOn)
                velocity -= gravity * deltaTime;
            baseOffset += velocity * deltaTime;
            if (baseOffset > 0f || velocity > 0f)
                return false;
            baseOffset = 0f;
            velocity = 0f;
            return true;
        }

        public static void Update(object manager)
        {
            if (Interlocked.Exchange(ref started, 1) == 0)
            {
                Thread worker = new Thread(ReadLauncher);
                worker.IsBackground = true;
                worker.Name = "WAL direct input client";
                worker.Start();
                PatchGame();
            }

            if (inputDataField == null)
            {
                inputDataField = manager.GetType().GetField("m_InputData", BindingFlags.Instance | BindingFlags.NonPublic);
                if (inputDataField == null)
                    return;
            }

            Array data = inputDataField.GetValue(manager) as Array;
            if (data == null || data.Length < 50)
                return;

            byte[] current = frame;
            bool[] pressed = new bool[50];
            for (int player = 0; player < 4; player++)
            {
                int offset = player * 16;
                uint buttons = ReadU32(current, offset);
                short lx = ReadI16(current, offset + 4);
                short ly = ReadI16(current, offset + 6);
                // GameInput: each control is P0 (combined) followed by P1..P4, and a
                // pawn reads root + its 1-based controller index.
                int p = player + 1;
                pressed[10 + p] = (buttons & (1u << 12)) != 0;
                pressed[15 + p] = (buttons & 1u) != 0 || ly < -12000;
                pressed[20 + p] = (buttons & 2u) != 0 || ly > 12000;
                pressed[25 + p] = (buttons & 4u) != 0 || lx < -12000;
                pressed[30 + p] = (buttons & 8u) != 0 || lx > 12000;
                pressed[35 + p] = (buttons & (1u << 4)) != 0;
                pressed[40 + p] = (buttons & (1u << 5)) != 0;
                pressed[45 + p] = (buttons & (1u << 6)) != 0;
                // Coin0 (index 4) is the game's shared/combined coin input. The four
                // player coin inputs are Coin1..Coin4 at indices 5..8.
                pressed[4 + p] = (buttons & (1u << 13)) != 0;
            }
            uint p1 = ReadU32(current, 0);
            pressed[0] = (p1 & (1u << 15)) != 0;
            pressed[1] = (p1 & (1u << 14)) != 0;
            ReadVolumeButtons(p1, pressed);

            // These are the roots used by IOManager.CombinedInputs. Keep each root's
            // desired state equal to the OR of its five player inputs so the game's
            // combiner sees a stable aggregate instead of treating it as P1's state.
            for (int r = 0; r < combinedRoots.Length; r++)
            {
                int root = combinedRoots[r];
                bool aggregate = false;
                for (int playerInput = root; playerInput <= root + 4; playerInput++)
                    aggregate |= pressed[playerInput];
                pressed[root] = aggregate;
            }

            if (!switchStateInitialized)
            {
                for (int i = 0; i < 50; i++)
                {
                    object item = data.GetValue(i);
                    if (item == null)
                        continue;
                    uint count = (uint)item.GetType().GetField("switchCnt", BindingFlags.Instance | BindingFlags.Public).GetValue(item);
                    switchCounts[i] = count;
                    switchStates[i] = (count & 1u) != 0;
                }
                switchStateInitialized = true;
            }

            for (int i = 0; i < 50; i++)
            {
                object item = data.GetValue(i);
                if (item == null)
                    continue;
                Type type = item.GetType();
                if (switchCountField == null)
                {
                    switchCountField = type.GetField("switchCnt", BindingFlags.Instance | BindingFlags.Public);
                    switchCountLastField = type.GetField("switchCntLast", BindingFlags.Instance | BindingFlags.Public);
                    inputTimeField = type.GetField("time", BindingFlags.Instance | BindingFlags.Public);
                    if (switchCountField == null || switchCountLastField == null || inputTimeField == null)
                        return;
                }
                uint count = (uint)switchCountField.GetValue(item);
                // With RIO disabled, the game no longer advances switchCntLast or time.
                // Reproduce its Rio poll: snapshot the old count, write the accumulated
                // edge count, and keep time in state for hold controls such as TEST.
                switchCountLastField.SetValue(item, count);
                if (switchStates[i] != pressed[i])
                {
                    if (Environment.GetEnvironmentVariable("WAL_TMNT_INPUT_TRACE") == "1")
                        Console.WriteLine("WAL TMNT switch {0}: {1} -> {2}", i, switchStates[i] ? 1 : 0, pressed[i] ? 1 : 0);
                    switchCounts[i]++;
                    switchStates[i] = pressed[i];
                    inputTimeField.SetValue(item, 0f);
                }
                else if (!IsCombinedRoot(i))
                {
                    float time = (float)inputTimeField.GetValue(item);
                    inputTimeField.SetValue(item, time + DeltaTime());
                }
                switchCountField.SetValue(item, switchCounts[i]);
            }
        }

        // Patches through the dump's Harmony (BepInEx 0Harmony 2.9), by reflection.
        //
        // Dumps patched before the game's own UnityEngine.Input reads were removed from
        // Assembly-CSharp still read the real keyboard (level select: W starts a level).
        // With the launcher's copy of this assembly, Unity's key wrappers read as released;
        // the game's input comes from the launcher only.
        //
        // The game's UnityNatives helpers (the cabinet's native library, absent from the
        // dump) are prefixed by the bundled plugin, which forwards them to its host process.
        // Without that host each one returns its default: no wave ever completes (the level
        // never scrolls on after the first fight), players take no damage, enemies never
        // attack. Postfixes run after that prefix and give the game's results.
        private static void PatchGame()
        {
            try
            {
                Type harmony = null, harmonyMethod = null, input = null, natives = null;
                foreach (Assembly assembly in AppDomain.CurrentDomain.GetAssemblies())
                {
                    harmony = harmony ?? assembly.GetType("HarmonyLib.Harmony", false);
                    harmonyMethod = harmonyMethod ?? assembly.GetType("HarmonyLib.HarmonyMethod", false);
                    input = input ?? assembly.GetType("UnityEngine.Input", false);
                    natives = natives ?? assembly.GetType("UnityNatives", false);
                }
                if (harmony == null || harmonyMethod == null)
                {
                    Console.WriteLine("WAL TMNT: no Harmony, real keyboard not released, no UnityNatives");
                    return;
                }
                object instance = Activator.CreateInstance(harmony, new object[] { "wal.directinput" });
                MethodInfo patch = null;
                foreach (MethodInfo method in harmony.GetMethods(BindingFlags.Public | BindingFlags.Instance))
                {
                    ParameterInfo[] parameters = method.GetParameters();
                    if (method.Name == "Patch" && parameters.Length >= 3 &&
                        parameters[0].ParameterType == typeof(MethodBase) && parameters[1].ParameterType == harmonyMethod &&
                        parameters[2].ParameterType == harmonyMethod)
                        patch = method;
                }
                if (patch == null)
                    return;

                if (input != null)
                {
                    object prefix = Activator.CreateInstance(harmonyMethod, new object[] { typeof(InputBridge).GetMethod("KeyReleased") });
                    foreach (MethodInfo key in input.GetMethods(BindingFlags.Public | BindingFlags.Static))
                    {
                        if ((key.Name != "GetKey" && key.Name != "GetKeyDown" && key.Name != "GetKeyUp") ||
                            key.GetParameters().Length != 1)
                            continue;
                        object[] args = new object[patch.GetParameters().Length];
                        args[0] = key;
                        args[1] = prefix;
                        patch.Invoke(instance, args);
                    }
                    Console.WriteLine("WAL TMNT: real keyboard released");
                }

                if (natives != null)
                {
                    int patched = 0;
                    foreach (MethodInfo native in natives.GetMethods(BindingFlags.Public | BindingFlags.Static))
                    {
                        MethodInfo result = typeof(GameNatives).GetMethod(native.Name, BindingFlags.Public | BindingFlags.Static);
                        if (result == null)
                            continue;
                        object[] args = new object[patch.GetParameters().Length];
                        args[0] = native;
                        args[2] = Activator.CreateInstance(harmonyMethod, new object[] { result });
                        patch.Invoke(instance, args);
                        patched++;
                    }
                    Console.WriteLine("WAL TMNT: {0} UnityNatives helpers", patched);
                }
            }
            catch (Exception e)
            {
                Console.WriteLine("WAL TMNT: game patches failed: " + e);
            }
        }

        public static bool KeyReleased(ref bool __result)
        {
            __result = false;
            return false;
        }

        private static bool IsCombinedRoot(int input)
        {
            for (int i = 0; i < combinedRoots.Length; i++)
                if (combinedRoots[i] == input)
                    return true;
            return false;
        }

        // Cabinet volume from the launcher's player 1 buttons named in WAL_MAP
        // (`b7=volup,b8=voldown`), never from the real keyboard.
        private static void ReadVolumeButtons(uint p1, bool[] pressed)
        {
            if (volumeUpMask == uint.MaxValue)
            {
                volumeUpMask = 0;
                volumeDownMask = 0;
                string map = Environment.GetEnvironmentVariable("WAL_MAP") ?? "";
                foreach (string pair in map.Split(','))
                {
                    string[] parts = pair.Split('=');
                    if (parts.Length != 2)
                        continue;
                    uint mask = ButtonMask(parts[0].Trim());
                    foreach (string native in parts[1].Split('+'))
                    {
                        string name = native.Trim().ToLowerInvariant();
                        if (name == "volup")
                            volumeUpMask |= mask;
                        else if (name == "voldown")
                            volumeDownMask |= mask;
                    }
                }
            }
            pressed[2] = (p1 & volumeUpMask) != 0;
            pressed[3] = (p1 & volumeDownMask) != 0;
        }

        private static uint ButtonMask(string name)
        {
            string[] names = { "up", "down", "left", "right", "b1", "b2", "b3", "b4", "b5", "b6", "b7", "b8",
                               "start", "coin", "service", "test" };
            for (int i = 0; i < names.Length; i++)
                if (string.Equals(names[i], name, StringComparison.OrdinalIgnoreCase))
                    return 1u << i;
            return 0;
        }

        private static float DeltaTime()
        {
            try
            {
                if (deltaTimeProperty == null)
                {
                    foreach (Assembly assembly in AppDomain.CurrentDomain.GetAssemblies())
                    {
                        Type time = assembly.GetType("UnityEngine.Time", false);
                        if (time == null)
                            continue;
                        deltaTimeProperty = time.GetProperty("deltaTime", BindingFlags.Public | BindingFlags.Static);
                        if (deltaTimeProperty != null)
                            break;
                    }
                }
                if (deltaTimeProperty != null)
                    return (float)deltaTimeProperty.GetValue(null, null);
            }
            catch { }
            return 1f / 60f;
        }

        private static uint ReadU32(byte[] data, int offset)
        {
            return (uint)(data[offset] | data[offset + 1] << 8 | data[offset + 2] << 16 | data[offset + 3] << 24);
        }

        private static short ReadI16(byte[] data, int offset)
        {
            return unchecked((short)(data[offset] | data[offset + 1] << 8));
        }

        private static void ReadLauncher()
        {
            int port = 33700;
            int parsed;
            if (int.TryParse(Environment.GetEnvironmentVariable("WAL_PORT"), out parsed))
                port = parsed;
            while (true)
            {
                try
                {
                    using (TcpClient client = new TcpClient())
                    {
                        client.Connect("127.0.0.1", port);
                        NetworkStream stream = client.GetStream();
                        byte[] hello = { 1, 0, 12, 0, 1, 0, (byte)'r', (byte)'a', (byte)'w', (byte)'t', (byte)'h', (byte)'r', (byte)'i', (byte)'l', (byte)'l', (byte)'s' };
                        stream.Write(hello, 0, hello.Length);
                        while (true)
                        {
                            byte[] header = ReadExact(stream, 4);
                            int length = header[2] | header[3] << 8;
                            byte[] payload = ReadExact(stream, length);
                            if (header[0] == 2 && length >= 64)
                                frame = payload;
                        }
                    }
                }
                catch
                {
                    Thread.Sleep(500);
                }
            }
        }

        private static byte[] ReadExact(Stream stream, int length)
        {
            byte[] data = new byte[length];
            int offset = 0;
            while (offset < length)
            {
                int count = stream.Read(data, offset, length - offset);
                if (count == 0)
                    throw new EndOfStreamException();
                offset += count;
            }
            return data;
        }
    }

    // Harmony postfixes for the game's UnityNatives helpers, by method and parameter name
    // (InputBridge.PatchGame). The cabinet library is absent, so these follow what each
    // caller in Assembly-CSharp expects. GetNewHeight is not here: Pawn.UpdateJump calls
    // InputBridge.GetNewHeight with the game's gravity.
    public static class GameNatives
    {
        // Wave.Update, once all of a wave's spawns are out: the wave ends (GO arrow, camera
        // released) when its enemies are dead, or at once for waves that end when all are
        // spawned. Enemies dropped in by Level.DropInEnemies get a moment to be counted.
        public static void CheckWaveComplete(float timeSinceDropIn, int numAliveEnemies, bool endWhenAllSpawned,
                                             ref bool __result)
        {
            __result = timeSinceDropIn >= 0.5f && (endWhenAllSpawned || numAliveEnemies <= 0);
        }

        // EnemyPawn.CanAttack, after the game's own cap on enemies attacking one turtle.
        // The player's time ratio is the credit's play time against the operator's target
        // (1 and over: past it). Before that, a player under half health who was hit in the
        // last 4 seconds gets a breather from enemies that hit recently.
        public static void AllowAttack(float dmgLast5, float playerHealthRatio, float playerTimeRatio,
                                       float timeSinceDamageDealt, ref bool __result)
        {
            if (playerTimeRatio >= 1f)
                __result = true;
            else if (dmgLast5 > 0f && playerHealthRatio < 0.5f)
                __result = timeSinceDamageDealt > 3f;
            else
                __result = true;
        }

        // Damage taken by a player: the operator's modifier, rising up to twice that once
        // the credit's play time goes past the cycle control time.
        public static void CurrentDamageMod(float cycleTime, float cycleControlTime, float gameDamageMod,
                                            ref float __result)
        {
            float over = cycleControlTime > 0f ? (cycleTime - cycleControlTime) / cycleControlTime : 0f;
            __result = gameDamageMod * (1f + Math.Max(0f, Math.Min(1f, over)));
        }

        // Hits in a row before a boss turns invincible (default 5), fewer as it weakens.
        public static void BossInvincibilityHits(float healthIn, float maxHealthIn, ref int __result)
        {
            float ratio = maxHealthIn > 0f ? healthIn / maxHealthIn : 1f;
            __result = ratio > 0.66f ? 5 : ratio > 0.33f ? 4 : 3;
        }

        // Player throws (target from AcquireTarget within 3 units): a close Foot soldier
        // in front, to the left or right. The game then checks the room behind the
        // player for the over-the-shoulder throw, else tries the throw into the screen.
        public static void ThrowCheck(float dist, float angleFromForward, float angleFromRight, ref bool __result)
        {
            __result = CanThrow(dist, angleFromForward, angleFromRight);
        }

        public static void ThrowCheckScreen(float dist, float angleFromForward, float angleFromRight, ref bool __result)
        {
            __result = CanThrow(dist, angleFromForward, angleFromRight);
        }

        private static bool CanThrow(float dist, float angleFromForward, float angleFromRight)
        {
            return dist <= 1.5f && angleFromForward <= 45f && (angleFromRight <= 45f || angleFromRight >= 135f);
        }

        // Enemy grab from behind (same facing as the turtle), Mouser grab from the front
        // (facing the turtle); the game moves the enemy 0.75 units from the turtle.
        public static void RestrainCheck(float dist, float angleBetweenForwards, ref bool __result)
        {
            __result = dist <= 1.5f && angleBetweenForwards <= 60f;
        }

        public static void RestrainCheckMouser(float dist, float angleBetweenForwards, ref bool __result)
        {
            __result = dist <= 1.5f && angleBetweenForwards <= 60f;
        }

        // Thrown objects settle on their second bounce: flat on the nearest quarter turn
        // (eulerAngles: 0..360). A rectangular box lies on its long side.
        public static void GetRotZTarget(float newEulerZ, ref float __result)
        {
            __result = (float)Math.Round(newEulerZ / 90f) * 90f;
        }

        public static void GetRotXTarget(bool rectShape, float newEulerX, ref float __result)
        {
            if (rectShape)
                __result = newEulerX >= 180f ? 360f : 0f;
            else
                __result = (float)Math.Round(newEulerX / 90f) * 90f;
        }

        // The turtle's ground circle stays on the ground during jumps: the pawn's base
        // offset is original + jump height + elevator; mod raises it to nearby NYC ground.
        public static void GetCircleY(float startY, float offset, float originalOffset, float elevatorOffset, float mod,
                                      ref float __result)
        {
            float jump = Math.Max(0f, offset - originalOffset - elevatorOffset);
            __result = startY - jump + mod;
        }

        // PlayerPawn turning to its target: at most its turn speed (degrees per second).
        public static void GetAngleDelta(float maxSpeed, float deltaT, float angle, ref float __result)
        {
            __result = Math.Min(maxSpeed * deltaT, angle);
        }
    }
}
