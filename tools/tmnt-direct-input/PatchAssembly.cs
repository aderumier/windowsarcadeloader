using System;
using System.IO;
using System.Linq;
using Mono.Cecil;
using Mono.Cecil.Cil;

public static class PatchAssembly
{
    public static int Main(string[] args)
    {
        if (args.Length != 3)
        {
            Console.Error.WriteLine("usage: PatchAssembly <Assembly-CSharp.dll> <backup.dll> <WAL.DirectInput.dll>");
            return 2;
        }
        string target = Path.GetFullPath(args[0]);
        string backup = Path.GetFullPath(args[1]);
        string helper = Path.GetFullPath(args[2]);
        if (!File.Exists(backup))
            File.Copy(target, backup);

        DefaultAssemblyResolver resolver = new DefaultAssemblyResolver();
        resolver.AddSearchDirectory(Path.GetDirectoryName(target));
        using (AssemblyDefinition assembly = AssemblyDefinition.ReadAssembly(backup, new ReaderParameters { AssemblyResolver = resolver }))
        using (AssemblyDefinition helperAssembly = AssemblyDefinition.ReadAssembly(helper))
        {
            ModuleDefinition module = assembly.MainModule;
            TypeDefinition io = module.GetType("IOManager");
            if (io == null)
                throw new InvalidOperationException("IOManager type not found");
            MethodDefinition updateInput = io.Methods.Single(m => m.Name == "UpdateInput_Unity");
            MethodDefinition update = io.Methods.Single(m => m.Name == "Update" && !m.IsStatic);
            MethodDefinition useRio = io.Methods.Single(m => m.Name == "set_m_UseRio");
            MethodDefinition helperMethod = helperAssembly.MainModule.GetType("WALDirect.InputBridge").Methods.Single(m => m.Name == "Update");
            MethodReference imported = module.ImportReference(helperMethod);

            // Unity's normal input hook stays empty. Write the per-switch Rio counters
            // after the cabinet poll/disconnected bookkeeping and before the game's
            // combined-input pass folds the five player switches together.
            updateInput.Body.Instructions.Clear();
            updateInput.Body.ExceptionHandlers.Clear();
            updateInput.Body.Variables.Clear();
            updateInput.Body.Instructions.Add(Instruction.Create(OpCodes.Ret));
            updateInput.Body.MaxStackSize = 0;

            Instruction combineCall = update.Body.Instructions.Single(i =>
                i.OpCode == OpCodes.Call && i.Operand is MethodReference mr && mr.Name == "UpdateCombinedInputs");
            ILProcessor il = update.Body.GetILProcessor();
            il.InsertBefore(combineCall, Instruction.Create(OpCodes.Ldarg_0));
            il.InsertBefore(combineCall, Instruction.Create(OpCodes.Call, imported));
            update.Body.MaxStackSize = Math.Max(update.Body.MaxStackSize, 1);

            // Jump movement uses a native helper that the TP plugin forwards to its
            // host. Run it locally, using the game's configured gravity, so the pawn
            // can reach the minimum jump height, fall, and signal EndJump on landing.
            TypeDefinition pawn = module.GetType("Pawn");
            MethodDefinition updateJump = pawn.Methods.Single(m => m.Name == "UpdateJump");
            Instruction heightCall = updateJump.Body.Instructions.Single(i =>
                i.OpCode == OpCodes.Call && i.Operand is MethodReference mr && mr.Name == "GetNewHeight");
            MethodReference gameInstance = (MethodReference)pawn.Methods.Single(m => m.Name == "BeginJump")
                .Body.Instructions.Single(i => i.OpCode == OpCodes.Call &&
                    i.Operand is MethodReference mr && mr.Name == "get_Instance").Operand;
            FieldDefinition gravity = module.GetType("Game").Fields.Single(f => f.Name == "m_JumpGravity");
            MethodDefinition heightHelper = helperAssembly.MainModule.GetType("WALDirect.InputBridge")
                .Methods.Single(m => m.Name == "GetNewHeight");
            ILProcessor jumpIl = updateJump.Body.GetILProcessor();
            jumpIl.InsertBefore(heightCall, Instruction.Create(OpCodes.Call, gameInstance));
            jumpIl.InsertBefore(heightCall, Instruction.Create(OpCodes.Ldfld, gravity));
            heightCall.Operand = module.ImportReference(heightHelper);
            updateJump.Body.MaxStackSize = Math.Max(updateJump.Body.MaxStackSize, 5);

            // The game's input comes from IOManager alone: real keyboard, mouse buttons
            // and joysticks read through UnityEngine.Input read as released. Developer
            // shortcuts (test menus: I/K volume, J/L service/test, U/O navigation; level
            // select: W starts a level) collide with the launcher's player keys.
            foreach (TypeDefinition type in module.GetTypes())
            {
                foreach (MethodDefinition method in type.Methods)
                {
                    if (!method.HasBody)
                        continue;
                    ILProcessor methodIl = method.Body.GetILProcessor();
                    foreach (Instruction instruction in method.Body.Instructions.ToArray())
                    {
                        MethodReference call = instruction.Operand as MethodReference;
                        if (instruction.OpCode != OpCodes.Call || call == null ||
                            call.DeclaringType.FullName != "UnityEngine.Input" ||
                            call.Parameters.Count != 1 || !IsRealInput(call.Name))
                            continue;
                        // Consume the key/button/axis argument and return false or 0,
                        // preserving branch targets.
                        bool axis = call.ReturnType.FullName == "System.Single";
                        instruction.OpCode = OpCodes.Pop;
                        instruction.Operand = null;
                        methodIl.InsertAfter(instruction, axis
                            ? Instruction.Create(OpCodes.Ldc_R4, 0f)
                            : Instruction.Create(OpCodes.Ldc_I4_0));
                    }
                }
            }

            FieldDefinition rio = io.Fields.Single(f => f.Name == "<m_UseRio>k__BackingField");
            useRio.Body.Instructions.Clear();
            useRio.Body.ExceptionHandlers.Clear();
            useRio.Body.Variables.Clear();
            useRio.Body.Instructions.Add(Instruction.Create(OpCodes.Ldarg_0));
            useRio.Body.Instructions.Add(Instruction.Create(OpCodes.Ldc_I4_0));
            useRio.Body.Instructions.Add(Instruction.Create(OpCodes.Stfld, rio));
            useRio.Body.Instructions.Add(Instruction.Create(OpCodes.Ret));
            useRio.Body.MaxStackSize = 2;

            TypeDefinition linux = module.GetType("Sys_Linux");
            MethodDefinition udev = linux == null ? null : linux.Methods.SingleOrDefault(m => m.Name == "CreateUSBMountRules");
            if (udev == null)
                throw new InvalidOperationException("Sys_Linux.CreateUSBMountRules not found");
            udev.Body.Instructions.Clear();
            udev.Body.ExceptionHandlers.Clear();
            udev.Body.Variables.Clear();
            udev.Body.Instructions.Add(Instruction.Create(OpCodes.Ret));
            udev.Body.MaxStackSize = 0;

            string temp = target + ".wal-tmp";
            assembly.Write(temp);
            File.Copy(temp, target, true);
            File.Delete(temp);
        }
        Console.WriteLine("patched direct launcher input into " + target);
        return 0;
    }

    private static bool IsRealInput(string name)
    {
        return name == "GetKey" || name == "GetKeyDown" || name == "GetKeyUp" ||
               name == "GetButton" || name == "GetButtonDown" || name == "GetButtonUp" ||
               name == "GetMouseButton" || name == "GetMouseButtonDown" || name == "GetMouseButtonUp" ||
               name == "GetAxis" || name == "GetAxisRaw";
    }
}
