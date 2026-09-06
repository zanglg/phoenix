#![no_main]
#![no_std]

use core::panic::PanicInfo;

core::arch::global_asm!(include_str!("entry.S"));

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
