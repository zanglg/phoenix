#![no_std]
#![no_main]

core::arch::global_asm!(
    include_str!("cooperative.S"),
    include_str!("preempt.S"),
    include_str!("sleep.S")
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
