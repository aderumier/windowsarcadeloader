#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <TMNT game directory>" >&2
    exit 2
fi

game_dir=$(cd "$1" && pwd)
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
managed="$game_dir/TMNT_Data/Managed"
cecil=$(find /usr/lib/mono/gac/Mono.Cecil -name Mono.Cecil.dll -type f 2>/dev/null | sort -V | tail -n 1)
if [ -z "$cecil" ]; then
    echo "Mono.Cecil not found; install the Mono development tools" >&2
    exit 1
fi
helper="$managed/WAL.DirectInput.dll"
target="$managed/Assembly-CSharp.dll"
backup="$managed/Assembly-CSharp.dll.wal-original"
mcs -target:library -sdk:2 -out:"$helper" "$script_dir/tmnt-direct-input/InputBridge.cs"
mcs -target:exe -out:/tmp/wal-tmnt-patch-assembly.exe -r:"$cecil" "$script_dir/tmnt-direct-input/PatchAssembly.cs"
MONO_PATH="$(dirname "$cecil")" mono /tmp/wal-tmnt-patch-assembly.exe "$target" "$backup" "$helper"

input_plugin="$game_dir/BepInEx/plugins/WALInput.dll"
if [ -f "$input_plugin" ]; then
    if [ ! -f "$input_plugin.wal-original" ]; then
        cp "$input_plugin" "$input_plugin.wal-original"
    fi
    mv "$input_plugin" "$input_plugin.wal-disabled"
fi

tp_plugin="$game_dir/BepInEx/plugins/TMNTTPPlugin.dll"
tp_backup="$tp_plugin.wal-original"
mcs -target:exe -out:/tmp/wal-tmnt-patch-tp.exe -r:"$cecil" "$script_dir/tmnt-direct-input/PatchTmntTP.cs"
MONO_PATH="$(dirname "$cecil")" mono /tmp/wal-tmnt-patch-tp.exe "$tp_plugin" "$tp_backup"
