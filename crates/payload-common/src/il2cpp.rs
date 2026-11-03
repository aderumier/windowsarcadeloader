//! IL2CPP's embedding API (`GameAssembly.dll`'s `il2cpp_*` exports), for the Unity games built
//! with IL2CPP: their classes, fields and methods looked up by name, so the payload does not
//! depend on one build's addresses; [`Il2Cpp::replace`] puts a native function in place of a
//! method's code.
//!
//! The API is used once IL2CPP has started (`il2cpp_init`, before the first scene): from the
//! game's own threads, e.g. its first `DllImport` resolved (`mono_imports.rs`).
//!
//! IL2CPP compiles methods ahead of time to native functions taking their arguments (`this`
//! first for an instance method) and a last hidden `MethodInfo*` (unused but by generic code).
//! Methods with the same compiled body share one function (empty constructors, getters of a
//! field at the same offset): [`Il2Cpp::replace`] is for the methods of their own body.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::OnceLock;

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

pub type P = *mut c_void;

/// IL2CPP's exports used here.
pub struct Il2Cpp {
    domain_get: unsafe extern "C" fn() -> P,
    domain_get_assemblies: unsafe extern "C" fn(P, *mut usize) -> *const P,
    assembly_get_image: unsafe extern "C" fn(P) -> P,
    image_get_name: unsafe extern "C" fn(P) -> *const c_char,
    class_from_name: unsafe extern "C" fn(P, *const c_char, *const c_char) -> P,
    class_get_method_from_name: unsafe extern "C" fn(P, *const c_char, i32) -> P,
    class_get_field_from_name: unsafe extern "C" fn(P, *const c_char) -> P,
    field_get_offset: unsafe extern "C" fn(P) -> usize,
    resolve_icall: unsafe extern "C" fn(*const c_char) -> P,
}

/// IL2CPP, once the game has loaded `GameAssembly.dll`.
pub fn il2cpp() -> Option<&'static Il2Cpp> {
    static IL2CPP: OnceLock<Il2Cpp> = OnceLock::new();
    if let Some(i) = IL2CPP.get() {
        return Some(i);
    }
    let module = unsafe { GetModuleHandleW(windows_sys::w!("GameAssembly.dll")) };
    if module.is_null() {
        return None;
    }
    let get = |name: &str| {
        let name = CString::new(name).unwrap();
        unsafe { GetProcAddress(module, name.as_ptr() as *const u8) }.map(|f| f as usize)
    };
    let i = unsafe {
        Il2Cpp {
            domain_get: std::mem::transmute(get("il2cpp_domain_get")?),
            domain_get_assemblies: std::mem::transmute(get("il2cpp_domain_get_assemblies")?),
            assembly_get_image: std::mem::transmute(get("il2cpp_assembly_get_image")?),
            image_get_name: std::mem::transmute(get("il2cpp_image_get_name")?),
            class_from_name: std::mem::transmute(get("il2cpp_class_from_name")?),
            class_get_method_from_name: std::mem::transmute(get("il2cpp_class_get_method_from_name")?),
            class_get_field_from_name: std::mem::transmute(get("il2cpp_class_get_field_from_name")?),
            field_get_offset: std::mem::transmute(get("il2cpp_field_get_offset")?),
            resolve_icall: std::mem::transmute(get("il2cpp_resolve_icall")?),
        }
    };
    Some(IL2CPP.get_or_init(|| i))
}

/// Offset of an array's first element (Il2CppArray: class, monitor, bounds, length).
pub const ARRAY_DATA: usize = 4 * size_of::<usize>();
/// Offset of an array's length.
pub const ARRAY_LENGTH: usize = 3 * size_of::<usize>();

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

impl Il2Cpp {
    /// The image of an assembly (`Assembly-CSharp`, `UnityEngine.InputLegacyModule`: its name
    /// without `.dll`), null if not loaded.
    pub fn image(&self, assembly: &str) -> P {
        let wanted = format!("{assembly}.dll");
        let mut count = 0;
        let list = unsafe { (self.domain_get_assemblies)((self.domain_get)(), &mut count) };
        if list.is_null() {
            return std::ptr::null_mut();
        }
        for k in 0..count {
            let image = unsafe { (self.assembly_get_image)(*list.add(k)) };
            let name = unsafe { (self.image_get_name)(image) };
            if !name.is_null() && unsafe { CStr::from_ptr(name) }.to_bytes().eq_ignore_ascii_case(wanted.as_bytes()) {
                return image;
            }
        }
        std::ptr::null_mut()
    }

    /// A class of an image (null if absent).
    pub fn class(&self, image: P, namespace: &str, name: &str) -> P {
        if image.is_null() {
            return std::ptr::null_mut();
        }
        unsafe { (self.class_from_name)(image, c(namespace).as_ptr(), c(name).as_ptr()) }
    }

    /// The compiled code of a class's method with `args` arguments (0: absent).
    pub fn method(&self, class: P, name: &str, args: i32) -> usize {
        if class.is_null() {
            return 0;
        }
        let method = unsafe { (self.class_get_method_from_name)(class, c(name).as_ptr(), args) };
        if method.is_null() {
            return 0;
        }
        // MethodInfo: its code first
        unsafe { *(method as *const usize) }
    }

    /// A field's offset in its class's objects (None if absent).
    pub fn offset(&self, class: P, name: &str) -> Option<usize> {
        if class.is_null() {
            return None;
        }
        let field = unsafe { (self.class_get_field_from_name)(class, c(name).as_ptr()) };
        (!field.is_null()).then(|| unsafe { (self.field_get_offset)(field) })
    }

    /// An internal call's native function (`UnityEngine.Input::GetKeyInt(UnityEngine.KeyCode)`),
    /// 0 if unknown.
    pub fn icall(&self, signature: &str) -> usize {
        unsafe { (self.resolve_icall)(c(signature).as_ptr()) as usize }
    }

    /// Puts `target` in place of the compiled method at `code` ([`Il2Cpp::method`]): it starts
    /// with a jump to it, every caller ends there. `target` takes the method's arguments and its
    /// hidden `MethodInfo*`, with the platform's calling convention.
    pub fn replace(&self, code: usize, target: usize) -> bool {
        if code == 0 {
            return false;
        }
        let code = code as *mut u8;
        // jmp [rip+0]; the target's address
        let mut jump = [0xFF, 0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        jump[6..].copy_from_slice(&(target as u64).to_le_bytes());
        use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
        use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut old = 0;
        unsafe {
            if VirtualProtect(code.cast(), jump.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                return false;
            }
            std::ptr::copy_nonoverlapping(jump.as_ptr(), code, jump.len());
            VirtualProtect(code.cast(), jump.len(), old, &mut old);
            FlushInstructionCache(GetCurrentProcess(), code.cast(), jump.len());
        }
        true
    }
}
