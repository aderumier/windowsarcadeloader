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
        private static MethodInfo getKeyMethod;
        private static object volumeUpKey;
        private static object volumeDownKey;
        private static readonly bool[] switchStates = new bool[50];
        private static readonly uint[] switchCounts = new uint[50];
        private static bool switchStateInitialized;
        private static readonly int[] combinedRoots = { 4, 10, 15, 20, 25, 30, 35, 40, 45 };

        // Pawn.UpdateJump expects true only when the pawn has landed. The bundled
        // TeknoParrot implementation delegates this calculation to its host process;
        // without that host it leaves height/velocity unchanged and returns false.
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
                int p = player + 1;
                pressed[10 + p] = (buttons & (1u << 12)) != 0;
                pressed[14 + p] = (buttons & 1u) != 0 || ly < -12000;
                pressed[19 + p] = (buttons & 2u) != 0 || ly > 12000;
                pressed[24 + p] = (buttons & 4u) != 0 || lx < -12000;
                pressed[29 + p] = (buttons & 8u) != 0 || lx > 12000;
                pressed[34 + p] = (buttons & (1u << 4)) != 0;
                pressed[39 + p] = (buttons & (1u << 5)) != 0;
                pressed[44 + p] = (buttons & (1u << 6)) != 0;
                // Coin0 (index 4) is the game's shared/combined coin input. The four
                // player coin inputs are Coin1..Coin4 at indices 5..8.
                pressed[4 + p] = (buttons & (1u << 13)) != 0;
            }
            uint p1 = ReadU32(current, 0);
            pressed[0] = (p1 & (1u << 15)) != 0;
            pressed[1] = (p1 & (1u << 14)) != 0;
            ReadVolumeKeys(pressed);

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

        private static bool IsCombinedRoot(int input)
        {
            for (int i = 0; i < combinedRoots.Length; i++)
                if (combinedRoots[i] == input)
                    return true;
            return false;
        }

        private static void ReadVolumeKeys(bool[] pressed)
        {
            if (getKeyMethod == null)
            {
                foreach (Assembly assembly in AppDomain.CurrentDomain.GetAssemblies())
                {
                    Type input = assembly.GetType("UnityEngine.Input", false);
                    if (input == null)
                        continue;
                    foreach (MethodInfo method in input.GetMethods(BindingFlags.Public | BindingFlags.Static))
                    {
                        ParameterInfo[] parameters = method.GetParameters();
                        if (method.Name != "GetKey" || parameters.Length != 1 ||
                            parameters[0].ParameterType.FullName != "UnityEngine.KeyCode")
                            continue;
                        getKeyMethod = method;
                        volumeUpKey = Enum.Parse(parameters[0].ParameterType, "PageUp");
                        volumeDownKey = Enum.Parse(parameters[0].ParameterType, "PageDown");
                        break;
                    }
                    if (getKeyMethod != null)
                        break;
                }
            }
            if (getKeyMethod != null)
            {
                pressed[2] = (bool)getKeyMethod.Invoke(null, new object[] { volumeUpKey });
                pressed[3] = (bool)getKeyMethod.Invoke(null, new object[] { volumeDownKey });
            }
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
}
