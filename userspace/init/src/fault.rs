#![no_std]
#![no_main]

core::arch::global_asm!(include_str!("fault.S"));

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
