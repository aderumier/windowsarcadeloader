using System;
using System.IO;
using System.Linq;
using Mono.Cecil;
using Mono.Cecil.Cil;

public static class PatchTmntTP
{
    public static int Main(string[] args)
    {
        if (args.Length != 2)
        {
            Console.Error.WriteLine("usage: PatchTmntTP <TMNTTPPlugin.dll> <backup.dll>");
            return 2;
        }

        string target = Path.GetFullPath(args[0]);
        string backup = Path.GetFullPath(args[1]);
        if (!File.Exists(backup))
            File.Copy(target, backup);

        using (AssemblyDefinition assembly = AssemblyDefinition.ReadAssembly(backup))
        {
            TypeDefinition ioManager = assembly.MainModule.GetType("BepInExTMNTTP.mIOManager");
            if (ioManager == null)
                throw new InvalidOperationException("BepInExTMNTTP.mIOManager type not found");
            TypeDefinition inputPatch = ioManager.NestedTypes.Single(t => t.Name == "UpdateInput_Unity");
            MethodDefinition poll = inputPatch.Methods.Single(m => m.Body != null && m.Body.Instructions.Any(i =>
                i.OpCode.Code == Code.Switch));
            // This Harmony postfix replays TeknoParrot's own switch mapping over the game's
            // input counters after the launcher bridge runs. Leave it inert so the direct
            // bridge is the sole writer, using the Linuxloader TMNT switch indices.
            poll.Body.Instructions.Clear();
            poll.Body.ExceptionHandlers.Clear();
            poll.Body.Variables.Clear();
            poll.Body.Instructions.Add(Instruction.Create(OpCodes.Ret));
            poll.Body.MaxStackSize = 0;

            string temp = target + ".wal-tmp";
            assembly.Write(temp);
            File.Copy(temp, target, true);
            File.Delete(temp);
        }

        Console.WriteLine("disabled the TeknoParrot input counter writer in " + target);
        return 0;
    }

}
