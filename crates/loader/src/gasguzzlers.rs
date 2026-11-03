//! `wal-gasguzzlers.exe`: Redline Rampage's race program, in place of the cabinet's
//! `Bin32\GasGuzzlers.exe`.
//!
//! The cabinet's executable is a small program wrapped by the HASP HL envelope: its code is
//! decrypted with the dongle, without which it stops ("Missing or Invalid Dongle (H0007)"). The
//! game is Gamepires' Gas Guzzlers on their Prana engine, whose DLLs (`Core.dll`, `Engine.dll`,
//! `Physics.dll`) and plug-ins (`*.rplugin`) are plain: this program does what the original
//! one does with them, through their exports.
//!
//! 1. The frontend's race selection, given on the command line (its `Launch_Game.am`: `-track
//!    613 -car 22,0.41,0.02,0.45 -gun 1258 -laps 2 ...`), is written to the `GVR_*` values of
//!    the engine's configuration file `GasGuzzlers.xml` (the game reads them with
//!    `CConfig::GetValue("GasGuzzlers", "GVR_...")`; the cabinet's file keeps the last race's),
//!    with the screen size of the `Engine` group.
//! 2. `InitializeCore(CCoreBootInfo*)`: the core (log, configuration file, script engine).
//! 3. A `CPranaEngine` in the engine's global `PE`, `Init(CPranaEngineBootInfo*)` (window,
//!    renderer, input, scenes, plug-in manager) with the program's instance and the
//!    configuration's window position and size. Then the sound, also the program's job:
//!    `CFMOD::Create()` (FMOD's event system) and `CFMOD::LoadProject("Audio/GasGuzzlers.fev")`
//!    (the music and effects; without them the race is silent).
//! 4. The plug-in manager searches the program's folder (`AddSearchPath`, relative to the
//!    project's data folder `Data/GasGuzzlers/`; `LoadPlugins`: the plug-ins register their
//!    types).
//! 5. A `CGame` (`GasGuzzlersPlugin.rplugin`) in the plug-in's global `Game`, given the engine
//!    (`CEngineClass::SetEngine`), `Init()`: the scenes, the race of the configuration.
//! 6. The race scene `GVRQuickRace` (`CSceneManager::SetCurrentSceneByName`; without a race on
//!    the command line, the configuration's `Engine/StartScene`), `CPranaEngine::Run()` until
//!    the game quits, then `CGame::DeInit()` and
//!    `CPranaEngine::DeInit()`.
//!
//! The objects are allocated here with room to spare (the classes' sizes are not exported:
//! `CPranaEngine` uses about 0x9e4 bytes, `CGame` 0x214) and never freed (the process ends).
//! Log: `wal-gasguzzlers.log` next to the program.

#![windows_subsystem = "windows"]
#![cfg_attr(not(windows), allow(dead_code))]

use std::ffi::{CString, c_void};
use std::io::Write;

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryA};

/// Space for the engine's objects, more than they use.
const ENGINE_SIZE: usize = 0x4000;
const GAME_SIZE: usize = 0x4000;
const BOOT_INFO_SIZE: usize = 0x1000;
const STRING_SIZE: usize = 0x100;

/// `CGame`'s virtual `Init()` (vtable slot 5) and `DeInit()` (slot 6).
const GAME_INIT_SLOT: usize = 5;
const GAME_DEINIT_SLOT: usize = 6;

fn log(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir().join("wal-gasguzzlers.log")) {
        let _ = writeln!(f, "{msg}");
    }
}

/// The program's folder (`Bin32`, the engine's application folder).
fn dir() -> std::path::PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default()
}

/// An export of a loaded module.
fn export(module: &str, name: &str) -> usize {
    let m = CString::new(module).unwrap();
    let h = unsafe { LoadLibraryA(m.as_ptr().cast()) };
    if h.is_null() {
        fail(&format!("cannot load {module}"));
    }
    let n = CString::new(name).unwrap();
    match unsafe { GetProcAddress(h, n.as_ptr().cast()) } {
        Some(f) => f as usize,
        None => fail(&format!("{module}: {name} not found")),
    }
}

fn fail(msg: &str) -> ! {
    log(&format!("error: {msg}"));
    std::process::exit(1);
}

/// A zeroed block that lives until the process ends.
fn block(size: usize) -> *mut c_void {
    Box::leak(vec![0u8; size].into_boxed_slice()).as_mut_ptr().cast()
}

type Ctor = unsafe extern "thiscall" fn(*mut c_void) -> *mut c_void;
type Method = unsafe extern "thiscall" fn(*mut c_void) -> u32;
type MethodPtr = unsafe extern "thiscall" fn(*mut c_void, *const c_void) -> u32;
type MethodBool = unsafe extern "thiscall" fn(*mut c_void, bool);

/// Calls a `Result f(const char*)` setter of a boot information object.
unsafe fn set_str(module: &str, name: &str, this: *mut c_void, value: &str) {
    let f: MethodPtr = unsafe { std::mem::transmute(export(module, name)) };
    let v = CString::new(value).unwrap();
    unsafe { f(this, v.as_ptr().cast()) };
}

/// The frontend's arguments -> `GVR_*` configuration values (name, value).
fn race_values(args: &[String]) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = vec![
        ("GVR_GreenLabel", "0".into()),
        ("GVR_NoBonusTrack", "0".into()),
        ("GVR_PlayerCount", "1".into()),
        ("GVR_Network_IsActive", "0".into()),
        ("GVR_Network_IsServer", "0".into()),
    ];
    let mut set = |name: &'static str, value: &str| {
        out.retain(|(n, _)| *n != name);
        out.push((name, value.to_string()));
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let fields = |names: &[&'static str], value: Option<&String>, set: &mut dyn FnMut(&'static str, &str)| {
            for (name, v) in names.iter().zip(value.map(|v| v.split(',')).into_iter().flatten()) {
                set(name, v.trim());
            }
        };
        match arg.as_str() {
            "-track" => fields(&["GVR_SelectedTrack"], it.next(), &mut set),
            "-car" => fields(&["GVR_CarID", "GVR_CarColorR", "GVR_CarColorG", "GVR_CarColorB"], it.next(), &mut set),
            "-driver" => fields(&["GVR_CarDriver"], it.next(), &mut set),
            "-plate" => fields(
                &["GVR_PlateR", "GVR_PlateG", "GVR_PlateB", "GVR_PlateTextR", "GVR_PlateTextG", "GVR_PlateTextB"],
                it.next(),
                &mut set,
            ),
            "-wheel" => fields(&["GVR_WheelID", "GVR_WheelR", "GVR_WheelG", "GVR_WheelB", "GVR_WheelReflection"], it.next(), &mut set),
            "-sticker" => fields(&["GVR_StickerID", "GVR_StickerR", "GVR_StickerG", "GVR_StickerB"], it.next(), &mut set),
            "-green" => set("GVR_GreenLabel", "1"),
            "-gun" => fields(&["GVR_CarGun"], it.next(), &mut set),
            "-music" => fields(&["GVR_Music"], it.next(), &mut set),
            "-nobonustrack" => set("GVR_NoBonusTrack", "1"),
            "-width" => fields(&["GVR_ScreenWidth"], it.next(), &mut set),
            "-height" => fields(&["GVR_ScreenHeight"], it.next(), &mut set),
            "-numai" => fields(&["GVR_NumberAI"], it.next(), &mut set),
            "-laps" => fields(&["GVR_Laps"], it.next(), &mut set),
            "-timer" => fields(&["GVR_Timer"], it.next(), &mut set),
            "-speedo" => fields(&["GVR_Speed_KPH"], it.next(), &mut set),
            // the cabinet's force feedback setting: 0 light, 1 medium, 2 heavy
            "-handling" => fields(&["GVR_Forcefeedback"], it.next(), &mut set),
            "-playernumber" => fields(&["GVR_PlayerNumber"], it.next(), &mut set),
            "-playercount" => fields(&["GVR_PlayerCount"], it.next(), &mut set),
            "-server" | "-client" => {
                set("GVR_Network_IsActive", "1");
                set("GVR_Network_IsServer", if arg == "-server" { "1" } else { "0" });
                fields(&["GVR_Network_IPAddress"], it.next(), &mut set);
            }
            other => log(&format!("argument {other:?} ignored")),
        }
    }
    out
}

/// Replaces the `value` of the `<Var name="...">` elements of the configuration text.
fn set_config_values(xml: &str, values: &[(&str, String)]) -> String {
    let mut xml = xml.to_string();
    for (name, value) in values {
        let key = format!("name=\"{name}\"");
        let Some(at) = xml.find(&key) else {
            log(&format!("{name}: not in the configuration"));
            continue;
        };
        let Some(v) = xml[at..].find("value=\"").map(|v| at + v + 7) else { continue };
        let Some(end) = xml[v..].find('"').map(|e| v + e) else { continue };
        xml.replace_range(v..end, value);
    }
    xml
}

/// The `value` of a `<Var name="...">` element of the configuration text.
fn config_value(xml: &str, name: &str) -> Option<String> {
    let at = xml.find(&format!("name=\"{name}\""))?;
    let v = xml[at..].find("value=\"").map(|v| at + v + 7)?;
    let end = xml[v..].find('"').map(|e| v + e)?;
    Some(xml[v..end].to_string())
}

fn main() {
    let dir = dir();
    let _ = std::env::set_current_dir(&dir);
    let args: Vec<String> = std::env::args().skip(1).collect();
    log(&format!("arguments {args:?}"));

    // 1. the race in the configuration file
    let config = dir.join("GasGuzzlers.xml");
    // the cabinet's race scene (cGVRQuickRaceScene: the configuration's race), else the
    // configuration's first scene (the PC game's menu, which the cabinet's data lacks)
    let race = args.iter().any(|a| a == "-track");
    let mut start_scene = String::from(if race { "GVRQuickRace" } else { "Menu" });
    // the window's position and size (the engine's `Engine` values)
    let mut window = [0i32, 0, 1024, 768];
    match std::fs::read(&config) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            if let Some(scene) = config_value(&text, "StartScene").filter(|_| !race) {
                start_scene = scene;
            }
            let mut values = race_values(&args);
            // the engine's screen: the frontend's size
            for (gvr, engine) in [("GVR_ScreenWidth", "ScreenWidth"), ("GVR_ScreenHeight", "ScreenHeight")] {
                if let Some(v) = values.iter().find(|(n, _)| *n == gvr).map(|(_, v)| v.clone()) {
                    values.push((engine, v));
                }
            }
            log(&format!("configuration {values:?}"));
            let text = set_config_values(&text, &values);
            for (i, name) in ["WindowPosX", "WindowPosY", "ScreenWidth", "ScreenHeight"].iter().enumerate() {
                if let Some(v) = config_value(&text, name).and_then(|v| v.trim().parse().ok()) {
                    window[i] = v;
                }
            }
            if let Err(e) = std::fs::write(&config, text) {
                log(&format!("cannot write {}: {e}", config.display()));
            }
        }
        Err(e) => log(&format!("cannot read {}: {e}", config.display())),
    }

    unsafe {
        // 2. the core
        let core_boot = block(BOOT_INFO_SIZE);
        let ctor: Ctor = std::mem::transmute(export("Core.dll", "??0CCoreBootInfo@@QAE@XZ"));
        ctor(core_boot);
        set_str("Core.dll", "?SetApplicationName@CCoreBootInfo@@QAE?AW4Result@@PBD@Z", core_boot, "Gas Guzzlers");
        set_str("Core.dll", "?SetApplicationNameShort@CCoreBootInfo@@QAE?AW4Result@@PBD@Z", core_boot, "GasGuzzlers");
        set_str("Core.dll", "?SetConfigFile@CCoreBootInfo@@QAE?AW4Result@@PBD@Z", core_boot, "GasGuzzlers.xml");
        let per_user: MethodBool = std::mem::transmute(export("Core.dll", "?SetUseConfigPerUser@CCoreBootInfo@@QAEX_N@Z"));
        per_user(core_boot, false);
        let init_core: unsafe extern "C" fn(*mut c_void) -> u32 =
            std::mem::transmute(export("Core.dll", "?InitializeCore@@YA?AW4Result@@PAVCCoreBootInfo@@@Z"));
        let r = init_core(core_boot);
        log(&format!("InitializeCore: {r}"));
        if r != 0 {
            fail("core initialization failed");
        }

        // 3. the engine
        let engine = block(ENGINE_SIZE);
        let ctor: Ctor = std::mem::transmute(export("Engine.dll", "??0CPranaEngine@@QAE@XZ"));
        ctor(engine);
        *(export("Engine.dll", "?PE@@3PAVCPranaEngine@@A") as *mut *mut c_void) = engine;
        let boot = block(BOOT_INFO_SIZE);
        let ctor: Ctor = std::mem::transmute(export("Engine.dll", "??0CPranaEngineBootInfo@@QAE@XZ"));
        ctor(boot);
        // the instance of the engine's window, its position and size (`Init` reads the
        // configuration's but uses these)
        let field = |offset: usize| boot.cast::<u8>().add(offset).cast::<i32>();
        *field(0x1c) = GetModuleHandleW(std::ptr::null()) as usize as i32;
        *field(0x28) = window[0];
        *field(0x2c) = window[1];
        *field(0x34) = window[2];
        *field(0x38) = window[3];
        log(&format!("window {window:?}"));
        let init: MethodPtr = std::mem::transmute(export("Engine.dll", "?Init@CPranaEngine@@QAE?AW4Result@@PAVCPranaEngineBootInfo@@@Z"));
        let r = init(engine, boot);
        log(&format!("CPranaEngine::Init: {r}"));
        if r != 0 {
            fail("engine initialization failed");
        }
        // the sound: the engine's FMOD (event system) and the game's event project (music,
        // effects), which the program creates (the engine and the game only use them)
        let create_fmod: unsafe extern "C" fn() -> *mut c_void = std::mem::transmute(export("Engine.dll", "?Create@CFMOD@@SAPAV1@XZ"));
        let fmod = create_fmod();
        let project = block(STRING_SIZE);
        let string: MethodPtr = std::mem::transmute(export("Core.dll", "??0CPranaString@@QAE@PBD@Z"));
        string(project, c"Audio/GasGuzzlers.fev".as_ptr().cast());
        let load_project: unsafe extern "C" fn(*const c_void) -> u32 =
            std::mem::transmute(export("Engine.dll", "?LoadProject@CFMOD@@SA_NABVCPranaString@@@Z"));
        log(&format!("CFMOD: {fmod:?}, project loaded {}", load_project(project) & 0xff));

        // 4. the plug-ins, from the program's folder
        let get_plugins: Method = std::mem::transmute(export("Engine.dll", "?GetPluginManager@CPranaEngine@@QBEPAVCPluginManager@@XZ"));
        let plugins = get_plugins(engine) as *mut c_void;
        let path = block(STRING_SIZE);
        string(path, c"../../Bin32".as_ptr().cast());
        let add_path: MethodPtr = std::mem::transmute(export("Engine.dll", "?AddSearchPath@CPluginManager@@QAE_NABVCPranaString@@@Z"));
        let r = add_path(plugins, path) & 0xff;
        let load: Method = std::mem::transmute(export("Engine.dll", "?LoadPlugins@CPluginManager@@QAEXXZ"));
        load(plugins);
        let count: Method = std::mem::transmute(export("Engine.dll", "?GetPluginCount@CPluginManager@@QBEIXZ"));
        log(&format!("plug-ins: search path added {r}, {} loaded", count(plugins)));

        // 5. the game
        let game = block(GAME_SIZE);
        let ctor: Ctor = std::mem::transmute(export("GasGuzzlersPlugin.rplugin", "??0CGame@@QAE@XZ"));
        ctor(game);
        // its engine (a `CEngineClass`: the renderer, input... through it)
        let set_engine: MethodPtr = std::mem::transmute(export("Engine.dll", "?SetEngine@CEngineClass@@QAE?AW4Result@@PAVCPranaEngine@@@Z"));
        set_engine(game, engine);
        *(export("GasGuzzlersPlugin.rplugin", "?Game@@3PAVCGame@@A") as *mut *mut c_void) = game;
        let vtable = *(game as *const *const usize);
        let game_init: Method = std::mem::transmute(*vtable.add(GAME_INIT_SLOT));
        let r = game_init(game);
        log(&format!("CGame::Init: {r}"));

        // the configuration's first scene
        let get_scenes: Method = std::mem::transmute(export("Engine.dll", "?GetSceneManager@CPranaEngine@@QAEPAVCSceneManager@@XZ"));
        let scenes = get_scenes(engine) as *mut c_void;
        let set_scene: unsafe extern "thiscall" fn(*mut c_void, *const u8, bool) -> u32 =
            std::mem::transmute(export("Engine.dll", "?SetCurrentSceneByName@CSceneManager@@QAE?AW4Result@@PBD_N@Z"));
        let name = CString::new(start_scene.clone()).unwrap();
        let r = set_scene(scenes, name.as_ptr().cast(), false);
        log(&format!("scene {start_scene}: {r}"));

        // 6. the engine's loop
        let run: Method = std::mem::transmute(export("Engine.dll", "?Run@CPranaEngine@@QAE?AW4Result@@XZ"));
        let r = run(engine);
        log(&format!("CPranaEngine::Run: {r}"));
        let game_deinit: Method = std::mem::transmute(*vtable.add(GAME_DEINIT_SLOT));
        game_deinit(game);
        let deinit: Method = std::mem::transmute(export("Engine.dll", "?DeInit@CPranaEngine@@UAE?AW4Result@@XZ"));
        deinit(engine);
        log("exit");
    }
}
