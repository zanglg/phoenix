#![no_std]
#![no_main]

core::arch::global_asm!(include_str!("child.S"));

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
