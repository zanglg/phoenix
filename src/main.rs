#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
mod arch;

mod mm;

#[cfg(target_os = "none")]
unsafe extern "C" {
    /// Start of kernel image in virtual address space (from linker script).
    static __kernel_virtual_start: u8;
    /// End of kernel image in virtual address space (from linker script).
    static __kernel_virtual_end: u8;
}

#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main() {
    use crate::arch::aarch64::boot;

    // Get kernel virtual addresses from linker script
    let kernel_virt_start = unsafe { &__kernel_virtual_start as *const u8 as u64 };
    let kernel_virt_end = unsafe { &__kernel_virtual_end as *const u8 as u64 };

    // Perform early initialization
    boot::early_init();

    // Perform main kernel initialization
    boot::kernel_init(kernel_virt_start, kernel_virt_end);
}

#[cfg(not(target_os = "none"))]
fn main() {
    println!("Hello from host!");
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    #[test]
    fn test_target_guard() {
        assert!(true);
    }
}
