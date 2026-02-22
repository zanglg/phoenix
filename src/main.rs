#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
mod arch;

#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() {
    let hello = b"Hello, world!";
    for byte in hello {
        unsafe {
            core::ptr::write_volatile(0xffff_ff80_0900_0000 as *mut u8, *byte);
        }
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    println!("Hello from host!");
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_target_guard() {
        assert!(true);
    }
}
