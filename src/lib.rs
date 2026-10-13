#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(improper_ctypes)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

/// A hash of the vendored VM's sources and build options (see `build.rs`): it changes
/// whenever the VM or its compiler does.
pub const SQUIRREL_SOURCE_HASH: &str = env!("SQUIRREL_SOURCE_HASH");

#[cfg(test)]
mod tests {
    use super::sq_open;
    use super::sq_close;

    #[test]
    fn build_check() {
        unsafe {
            let vm = sq_open(1024);
            sq_close(vm);
        }
    }
}
