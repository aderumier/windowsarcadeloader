//! Mono's embedding API (`mono-2.0-bdwgc.dll`'s exports, or the older Unity's `mono.dll`), for
//! the Unity games: classes, fields and methods of the game's managed code, looked up by name;
//! [`Mono::replace`] puts a native function in place of a method's compiled code.
//!
//! Mono's garbage collector (Boehm) does not move objects: an object's address stays valid
//! while it is referenced (a static field, a GC handle, a pointer on an attached thread's
//! stack). Calls are made from managed threads (the game's `DllImport` calls) or from threads
//! attached with [`Mono::attach`], never from `DllMain`.

use std::ffi::{CString, c_char, c_void};
use std::sync::OnceLock;

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

pub type P = *mut c_void;

/// Mono's exports used here.
pub struct Mono {
    get_root_domain: unsafe extern "C" fn() -> P,
    thread_attach: unsafe extern "C" fn(P) -> P,
    image_loaded: unsafe extern "C" fn(*const c_char) -> P,
    class_from_name: unsafe extern "C" fn(P, *const c_char, *const c_char) -> P,
    class_get_field_from_name: unsafe extern "C" fn(P, *const c_char) -> P,
    class_get_method_from_name: unsafe extern "C" fn(P, *const c_char, i32) -> P,
    class_vtable: unsafe extern "C" fn(P, P) -> P,
    vtable_get_static_field_data: unsafe extern "C" fn(P) -> P,
    field_get_offset: unsafe extern "C" fn(P) -> u32,
    field_static_set_value: unsafe extern "C" fn(P, P, P),
    field_set_value: unsafe extern "C" fn(P, P, P),
    object_new: unsafe extern "C" fn(P, P) -> P,
    object_get_class: unsafe extern "C" fn(P) -> P,
    runtime_object_init: unsafe extern "C" fn(P),
    runtime_invoke: unsafe extern "C" fn(P, P, *mut P, *mut P) -> P,
    gchandle_new: unsafe extern "C" fn(P, i32) -> u32,
    array_new: unsafe extern "C" fn(P, P, usize) -> P,
    get_byte_class: unsafe extern "C" fn() -> P,
    string_new: unsafe extern "C" fn(P, *const c_char) -> P,
    class_get_type: unsafe extern "C" fn(P) -> P,
    type_get_object: unsafe extern "C" fn(P, P) -> P,
    compile_method: unsafe extern "C" fn(P) -> P,
}

/// Mono, once the game has loaded it.
pub fn mono() -> Option<&'static Mono> {
    static MONO: OnceLock<Mono> = OnceLock::new();
    if let Some(m) = MONO.get() {
        return Some(m);
    }
    let mut module = unsafe { GetModuleHandleW(windows_sys::w!("mono-2.0-bdwgc.dll")) };
    if module.is_null() {
        module = unsafe { GetModuleHandleW(windows_sys::w!("mono.dll")) };
    }
    if module.is_null() {
        return None;
    }
    let get = |name: &str| {
        let name = CString::new(name).unwrap();
        unsafe { GetProcAddress(module, name.as_ptr() as *const u8) }.map(|f| f as usize)
    };
    let m = unsafe {
        Mono {
            get_root_domain: std::mem::transmute(get("mono_get_root_domain")?),
            thread_attach: std::mem::transmute(get("mono_thread_attach")?),
            image_loaded: std::mem::transmute(get("mono_image_loaded")?),
            class_from_name: std::mem::transmute(get("mono_class_from_name")?),
            class_get_field_from_name: std::mem::transmute(get("mono_class_get_field_from_name")?),
            class_get_method_from_name: std::mem::transmute(get("mono_class_get_method_from_name")?),
            class_vtable: std::mem::transmute(get("mono_class_vtable")?),
            vtable_get_static_field_data: std::mem::transmute(get("mono_vtable_get_static_field_data")?),
            field_get_offset: std::mem::transmute(get("mono_field_get_offset")?),
            field_static_set_value: std::mem::transmute(get("mono_field_static_set_value")?),
            field_set_value: std::mem::transmute(get("mono_field_set_value")?),
            object_new: std::mem::transmute(get("mono_object_new")?),
            object_get_class: std::mem::transmute(get("mono_object_get_class")?),
            runtime_object_init: std::mem::transmute(get("mono_runtime_object_init")?),
            runtime_invoke: std::mem::transmute(get("mono_runtime_invoke")?),
            gchandle_new: std::mem::transmute(get("mono_gchandle_new")?),
            array_new: std::mem::transmute(get("mono_array_new")?),
            get_byte_class: std::mem::transmute(get("mono_get_byte_class")?),
            string_new: std::mem::transmute(get("mono_string_new")?),
            class_get_type: std::mem::transmute(get("mono_class_get_type")?),
            type_get_object: std::mem::transmute(get("mono_type_get_object")?),
            compile_method: std::mem::transmute(get("mono_compile_method")?),
        }
    };
    Some(MONO.get_or_init(|| m))
}

/// Offset of an array's first element (MonoArray: vtable, synchronisation, bounds, length).
pub const ARRAY_DATA: usize = 4 * size_of::<usize>();
/// Offset of a boxed value (vtable, synchronisation).
pub const BOXED: usize = 2 * size_of::<usize>();

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

impl Mono {
    /// The root domain (null before Mono's start).
    pub fn domain(&self) -> P {
        unsafe { (self.get_root_domain)() }
    }

    /// Attaches the calling thread to the root domain (a thread of the payload's own).
    pub fn attach(&self) -> bool {
        let domain = self.domain();
        !domain.is_null() && !unsafe { (self.thread_attach)(domain) }.is_null()
    }

    /// The loaded assembly image named `name` (null until the game loads it).
    pub fn image(&self, name: &str) -> P {
        unsafe { (self.image_loaded)(c(name).as_ptr()) }
    }

    /// The class `namespace.name` of `image` (`Outer/Inner` for a nested class).
    pub fn class(&self, image: P, namespace: &str, name: &str) -> P {
        if image.is_null() {
            return image;
        }
        unsafe { (self.class_from_name)(image, c(namespace).as_ptr(), c(name).as_ptr()) }
    }

    pub fn field(&self, class: P, name: &str) -> P {
        if class.is_null() {
            return class;
        }
        unsafe { (self.class_get_field_from_name)(class, c(name).as_ptr()) }
    }

    /// The method `name` of `class` taking `args` arguments.
    pub fn method(&self, class: P, name: &str, args: i32) -> P {
        if class.is_null() {
            return class;
        }
        unsafe { (self.class_get_method_from_name)(class, c(name).as_ptr(), args) }
    }

    /// A field's offset in its object (from the object's start, header included), or in the
    /// class's static data.
    pub fn offset(&self, field: P) -> usize {
        unsafe { (self.field_get_offset)(field) as usize }
    }

    /// The class's vtable in the root domain. Its static constructor is not run here (it may
    /// call Unity, from the main thread only): the game runs it.
    pub fn vtable(&self, class: P) -> P {
        if class.is_null() {
            return class;
        }
        unsafe { (self.class_vtable)(self.domain(), class) }
    }

    /// Address of the static field `field` of `class` (zero until the game sets it).
    pub fn static_addr(&self, class: P, field: P) -> *mut u8 {
        let vtable = self.vtable(class);
        if vtable.is_null() || field.is_null() {
            return std::ptr::null_mut();
        }
        let data = unsafe { (self.vtable_get_static_field_data)(vtable) } as *mut u8;
        if data.is_null() {
            return data;
        }
        unsafe { data.add(self.offset(field)) }
    }

    pub fn static_set(&self, class: P, field: P, value: P) -> bool {
        let vtable = self.vtable(class);
        if vtable.is_null() || field.is_null() {
            return false;
        }
        unsafe { (self.field_static_set_value)(vtable, field, value) };
        true
    }

    /// Stores `value` (an object pointer, or a pointer to a value) in `obj`'s field.
    pub fn set(&self, obj: P, field: P, value: P) {
        unsafe { (self.field_set_value)(obj, field, value) };
    }

    /// A new object of `class`, its default constructor run.
    pub fn new_object(&self, class: P) -> P {
        if class.is_null() {
            return class;
        }
        let obj = unsafe { (self.object_new)(self.domain(), class) };
        if !obj.is_null() {
            unsafe { (self.runtime_object_init)(obj) };
        }
        obj
    }

    pub fn class_of(&self, obj: P) -> P {
        unsafe { (self.object_get_class)(obj) }
    }

    /// Calls `method` on `obj` (null: a static method); `args` point to the values (objects
    /// are passed as the object pointer itself). `None`: the method threw.
    pub fn invoke(&self, method: P, obj: P, args: &mut [P]) -> Option<P> {
        let mut exc: P = std::ptr::null_mut();
        let r = unsafe { (self.runtime_invoke)(method, obj, args.as_mut_ptr(), &mut exc) };
        exc.is_null().then_some(r)
    }

    /// Keeps `obj` alive (and in place) for the process's life.
    pub fn pin(&self, obj: P) -> P {
        if !obj.is_null() {
            unsafe { (self.gchandle_new)(obj, 1) };
        }
        obj
    }

    /// A new string of `text`.
    pub fn string(&self, text: &str) -> P {
        unsafe { (self.string_new)(self.domain(), c(text).as_ptr()) }
    }

    /// The `System.Type` object of `class`.
    pub fn type_object(&self, class: P) -> P {
        if class.is_null() {
            return class;
        }
        unsafe { (self.type_get_object)(self.domain(), (self.class_get_type)(class)) }
    }

    /// Puts `target` in place of `method`'s code: its compiled code (compiled now) starts with a
    /// jump to it, every caller ends there. `target` takes the method's arguments (`this` first
    /// for an instance method) and returns its value, with the platform's calling convention.
    /// Small methods (property getters...) may be inlined in their callers: only larger ones are
    /// replaced this way.
    pub fn replace(&self, method: P, target: usize) -> bool {
        if method.is_null() {
            return false;
        }
        let code = unsafe { (self.compile_method)(method) } as *mut u8;
        if code.is_null() {
            return false;
        }
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

    /// A new `byte[len]`.
    pub fn byte_array(&self, len: usize) -> P {
        unsafe { (self.array_new)(self.domain(), (self.get_byte_class)(), len) }
    }
}

/// Reads the `T` at `offset` of the object (or static data) at `base`.
///
/// # Safety
/// `base + offset` must hold a `T`.
pub unsafe fn read<T: Copy>(base: *const u8, offset: usize) -> T {
    unsafe { (base.add(offset) as *const T).read_unaligned() }
}

/// Writes `value` at `offset` of the object at `base`.
///
/// # Safety
/// `base + offset` must hold a `T`.
pub unsafe fn write<T: Copy>(base: *mut u8, offset: usize, value: T) {
    unsafe { (base.add(offset) as *mut T).write_unaligned(value) }
}

