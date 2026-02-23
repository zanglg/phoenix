use core::arch::global_asm;
use core::panic::PanicInfo;

global_asm!(include_str!("boot.S"));

pub mod address;
pub mod boot;
pub mod serial;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfe");
        }
    }
}
