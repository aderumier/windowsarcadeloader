//! AVPro Video's uGUI display without its player: the Windows build of an Android game ships
//! AVPro's Android backend only (`AndroidMediaPlayer`, no native plugin), its `DisplayUGUI` stays
//! empty (Big Foot Crush's attract video). [`play_with_unity`] replaces `DisplayUGUI`'s
//! `mainTexture` getter: a Unity `VideoPlayer` added to the `MediaPlayer`'s object plays its
//! file (`m_VideoPath`, `m_Loop`), its texture is the display's.
//!
//! The getter runs on Unity's main thread (the display's `LateUpdate`, the canvas' rebuild),
//! where Unity's API is called; `LateUpdate` sees the texture change and dirties the material.

use std::sync::{Mutex, OnceLock};

use crate::log;
use crate::mono::{Mono, P, mono, read};

/// `VideoSource.Url`.
const SOURCE_URL: i32 = 1;
/// `VideoRenderMode.APIOnly`: the frames in `VideoPlayer.texture`.
const RENDER_API_ONLY: i32 = 4;
/// `VideoAudioOutputMode.None`.
const AUDIO_NONE: i32 = 0;

/// The managed methods and fields used.
struct Api {
    media_player: usize,
    video_path: usize,
    looping: usize,
    game_object: P,
    add_component: P,
    video_player: P,
    set_source: P,
    set_url: P,
    set_looping: P,
    set_render_mode: P,
    set_audio_output: P,
    play: P,
    texture: P,
    streaming_assets: P,
    black: P,
}

unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<Api> = OnceLock::new();

/// The video players made: (the `MediaPlayer`, its `VideoPlayer`).
static PLAYERS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// A managed string's text.
unsafe fn text(string: P) -> String {
    if string.is_null() {
        return String::new();
    }
    // MonoString: the object's header (vtable, synchronisation), length, UTF-16 characters
    let header = 2 * size_of::<usize>();
    let base = string as *const u8;
    let len = unsafe { read::<i32>(base, header) }.max(0) as usize;
    let chars: Vec<u16> = (0..len).map(|i| unsafe { read::<u16>(base, header + 4 + 2 * i) }).collect();
    String::from_utf16_lossy(&chars)
}

/// The file's URL: a path relative to the streaming assets (AVPro's default location).
fn url(m: &Mono, api: &Api, path: &str) -> String {
    if path.contains(':') || path.starts_with('/') || path.starts_with('\\') {
        return path.to_string();
    }
    let root = m.invoke(api.streaming_assets, std::ptr::null_mut(), &mut []).unwrap_or(std::ptr::null_mut());
    format!("{}/{}", unsafe { text(root) }, path)
}

/// A `VideoPlayer` on the `MediaPlayer`'s object, playing its file.
fn new_player(m: &Mono, api: &Api, media: P) -> Option<P> {
    let object = m.invoke(api.game_object, media, &mut [])?;
    let player = m.invoke(api.add_component, object, &mut [m.type_object(api.video_player)])?;
    if player.is_null() {
        return None;
    }
    m.pin(player);
    let path = unsafe { text(read::<P>(media as *const u8, api.video_path)) };
    let looping = unsafe { read::<u8>(media as *const u8, api.looping) } as i32;
    let url = url(m, api, &path);
    let (mut source, mut mode, mut audio, mut looping) = (SOURCE_URL, RENDER_API_ONLY, AUDIO_NONE, looping);
    let int = |v: &mut i32| v as *mut i32 as P;
    m.invoke(api.set_source, player, &mut [int(&mut source)])?;
    m.invoke(api.set_render_mode, player, &mut [int(&mut mode)])?;
    m.invoke(api.set_audio_output, player, &mut [int(&mut audio)])?;
    m.invoke(api.set_looping, player, &mut [int(&mut looping)])?;
    m.invoke(api.set_url, player, &mut [m.string(&url)])?;
    m.invoke(api.play, player, &mut [])?;
    log!("avpro: {url} played by Unity's VideoPlayer (loop {looping})");
    Some(player)
}

/// `DisplayUGUI.mainTexture`: its `MediaPlayer`'s video player's frame (black until the first).
unsafe extern "C" fn main_texture(this: P) -> P {
    let (Some(m), Some(api)) = (mono(), API.get()) else { return std::ptr::null_mut() };
    let black = || m.invoke(api.black, std::ptr::null_mut(), &mut []).unwrap_or(std::ptr::null_mut());
    let media = unsafe { read::<P>(this as *const u8, api.media_player) };
    if media.is_null() {
        return black();
    }
    let mut players = PLAYERS.lock().unwrap();
    let known = players.iter().position(|(k, _)| *k == media as usize);
    if let Some(i) = known {
        // a destroyed player (the scene's change) throws: made again
        match m.invoke(api.texture, players[i].1 as P, &mut []) {
            Some(texture) if !texture.is_null() => return texture,
            Some(_) => return black(),
            None => {
                players.remove(i);
            }
        }
    }
    if let Some(player) = new_player(m, api, media) {
        players.push((media as usize, player as usize));
    } else {
        log!("avpro: no VideoPlayer for the display");
        // not tried again every frame
        players.push((media as usize, 0));
    }
    black()
}

/// Replaces `DisplayUGUI`'s texture once the game's assemblies are loaded (from a thread
/// attached to Mono). False: AVPro's or Unity's video classes not there.
pub fn play_with_unity(m: &Mono) -> bool {
    let avpro = m.image("Assembly-CSharp");
    let display = m.class(avpro, "RenderHeads.Media.AVProVideo", "DisplayUGUI");
    let media = m.class(avpro, "RenderHeads.Media.AVProVideo", "MediaPlayer");
    let core = m.image("UnityEngine.CoreModule");
    let component = m.class(core, "UnityEngine", "Component");
    let object = m.class(core, "UnityEngine", "GameObject");
    let application = m.class(core, "UnityEngine", "Application");
    let texture2d = m.class(core, "UnityEngine", "Texture2D");
    let video = m.class(m.image("UnityEngine.VideoModule"), "UnityEngine.Video", "VideoPlayer");
    let fields = [m.field(display, "_mediaPlayer"), m.field(media, "m_VideoPath"), m.field(media, "m_Loop")];
    let api = Api {
        media_player: m.offset(fields[0]),
        video_path: m.offset(fields[1]),
        looping: m.offset(fields[2]),
        game_object: m.method(component, "get_gameObject", 0),
        add_component: m.method(object, "AddComponent", 1),
        video_player: video,
        set_source: m.method(video, "set_source", 1),
        set_url: m.method(video, "set_url", 1),
        set_looping: m.method(video, "set_isLooping", 1),
        set_render_mode: m.method(video, "set_renderMode", 1),
        set_audio_output: m.method(video, "set_audioOutputMode", 1),
        play: m.method(video, "Play", 0),
        texture: m.method(video, "get_texture", 0),
        streaming_assets: m.method(application, "get_streamingAssetsPath", 0),
        black: m.method(texture2d, "get_blackTexture", 0),
    };
    let methods = [
        api.game_object,
        api.add_component,
        api.set_source,
        api.set_url,
        api.set_looping,
        api.set_render_mode,
        api.set_audio_output,
        api.play,
        api.texture,
        api.streaming_assets,
        api.black,
    ];
    if fields.iter().chain(methods.iter()).any(|p| p.is_null()) || video.is_null() {
        log!("avpro: AVPro's display or Unity's VideoPlayer not found");
        return false;
    }
    let _ = API.set(api);
    let replaced = m.replace(m.method(display, "get_mainTexture", 0), main_texture as *const () as usize);
    log!("avpro: the uGUI display's video from Unity's VideoPlayer ({replaced})");
    replaced
}
