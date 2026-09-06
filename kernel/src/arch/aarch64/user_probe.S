/* Minimal EL0 conformance probe.
 *
 * The program first verifies that an unknown syscall returns -ENOSYS, then
 * calls exit(42). It owns no runtime, relocations, writable data, or stack
 * frames. The kernel maps this page read/execute at USER_CODE_BASE.
 */

.section .user_probe.text, "ax"
.balign 4096

.global __user_probe_entry
.type __user_probe_entry, %function
__user_probe_entry:
    mov     x8, #99
    svc     #0

    mov     x9, #38
    neg     x9, x9
    cmp     x0, x9
    b.ne    .Luser_probe_failed

    mov     x0, #42
    mov     x8, #0
    svc     #0

.Luser_probe_failed:
    mov     x0, #1
    mov     x8, #0
    svc     #0

.Luser_probe_unreachable:
    b       .Luser_probe_unreachable

.size __user_probe_entry, . - __user_probe_entry

.section .user_probe.stack, "aw", @nobits
.balign 4096
.space 4096
