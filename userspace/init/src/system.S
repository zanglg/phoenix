/* Native 0.1 conformance init: file transactions, console input and child reap. */
.section .text.entry, "ax"
.balign 4096
.global _start
.type _start, %function
.macro call number
    mov x8, #\number
    svc #0
.endm
.macro expect value
    mov x9, #\value
    cmp x0, x9
    b.ne .Lfail
.endm
_start:
    sub sp, sp, #256
    mov x0, #1
    adr x1, greeting
    mov x2, #greeting_end-greeting
    call 1
    expect greeting_end-greeting
    /* Bad user pointers must neither print nor advance a file offset. */
    mov x0, #1
    mov x1, #0
    mov x2, #4
    call 1
    expect -14
    adr x0, motd
    mov x1, #8
    mov x2, #0
    call 3
    expect 3
    mov x19, x0
    mov x0, x19
    mov x1, #0
    mov x2, #32
    call 2
    expect -14
    /* A crossing write must validate every page before changing any byte. */
    mov x23, sp
    and x23, x23, #-4096
    add x23, x23, #4096
    sub x23, x23, #16
    mov x9, #0x1234
    stp x9, x9, [x23]
    mov x0, x19
    mov x1, x23
    mov x2, #34
    call 2
    expect -14
    ldr x9, [x23]
    mov x10, #0x1234
    cmp x9, x10
    b.ne .Lfail
    /* The matching crossing source must fail before emitting any bytes. */
    mov x0, #1
    mov x1, x23
    mov x2, #34
    call 1
    expect -14
    mov x0, x19
    mov x1, sp
    mov x2, #128
    call 2
    expect 34
    ldrb w9, [sp]
    cmp w9, #'P'
    b.ne .Lfail
    mov x0, #1
    mov x1, sp
    mov x2, #34
    call 1
    expect 34
    mov x0, x19
    call 4
    expect 0
    /* Exercise successful copies spanning two mapped stack pages. */
    adr x0, motd
    mov x1, #8
    mov x2, #0
    call 3
    expect 3
    mov x19, x0
    mov x23, sp
    and x23, x23, #-4096
    sub x23, x23, #16
    mov x0, x19
    mov x1, x23
    mov x2, #34
    call 2
    expect 34
    mov x0, #1
    mov x1, x23
    mov x2, #34
    call 1
    expect 34
    mov x0, x19
    call 4
    expect 0
    /* Create, write, close, reopen and read the same in-memory file. */
    adr x0, note
    mov x1, #8
    mov x2, #1
    call 3
    expect 3
    mov x19, x0
    mov x0, x19
    adr x1, payload
    mov x2, #8
    call 1
    expect 8
    mov x0, x19
    call 4
    expect 0
    adr x0, note
    mov x1, #8
    mov x2, #0
    call 3
    expect 3
    mov x19, x0
    mov x0, x19
    mov x1, sp
    mov x2, #32
    call 2
    expect 8
    ldr x9, [sp]
    adr x10, payload
    ldr x10, [x10]
    cmp x9, x10
    b.ne .Lfail
    mov x0, x19
    mov x1, sp
    mov x2, #32
    call 2
    expect 0
    mov x0, x19
    call 4
    expect 0
    /* A full tmp file rejects the whole next write and remains readable. */
    adr x0, full
    mov x1, #8
    mov x2, #1
    call 3
    expect 3
    mov x19, x0
    mov x20, #4
.Lfill:
    mov x0, x19
    mov x1, sp
    mov x2, #256
    call 1
    expect 256
    subs x20, x20, #1
    b.ne .Lfill
    mov x0, x19
    mov x1, sp
    mov x2, #1
    call 1
    expect -28
    mov x0, x19
    call 4
    expect 0
    adr x0, full
    mov x1, #8
    mov x2, #0
    call 3
    expect 3
    mov x19, x0
    mov x20, #4
.Lreadfull:
    mov x0, x19
    mov x1, sp
    mov x2, #256
    call 2
    expect 256
    subs x20, x20, #1
    b.ne .Lreadfull
    mov x0, x19
    mov x1, sp
    mov x2, #1
    call 2
    expect 0
    mov x0, x19
    call 4
    expect 0
    /* Fill the remaining six nodes, then verify ENOSPC without leaking an FD. */
    adr x21, file_names
    mov x22, #6
.Lnodes:
    mov x0, x21
    mov x1, #5
    mov x2, #1
    call 3
    expect 3
    call 4
    expect 0
    add x21, x21, #5
    subs x22, x22, #1
    b.ne .Lnodes
    mov x0, x21
    mov x1, #5
    mov x2, #1
    call 3
    expect -28
    /* Descriptor exhaustion and recovery must be deterministic. */
    mov x20, #3
.Lopen:
    adr x0, motd
    mov x1, #8
    mov x2, #0
    call 3
    cmp x0, x20
    b.ne .Lfail
    add x20, x20, #1
    cmp x20, #11
    b.ne .Lopen
    adr x0, motd
    mov x1, #8
    mov x2, #0
    call 3
    expect -24
    mov x20, #3
.Lclose:
    mov x0, x20
    call 4
    expect 0
    add x20, x20, #1
    cmp x20, #11
    b.ne .Lclose
    /* Poll nonblocking console input until the three-byte test line arrives. */
    mov x20, #0
.Linput:
    mov x0, #0
    add x1, sp, x20
    mov x2, #3
    sub x2, x2, x20
    call 2
    cmn x0, #11
    b.eq .Linput
    cmp x0, #0
    b.le .Lfail
    add x20, x20, x0
    cmp x20, #3
    b.lo .Linput
    ldrb w9, [sp]
    cmp w9, #'o'
    b.ne .Lfail
    ldrb w9, [sp, #1]
    cmp w9, #'k'
    b.ne .Lfail
    ldrb w9, [sp, #2]
    cmp w9, #10
    b.ne .Lfail
    /* Two normal runs must reuse process slot and frames while retaining x28. */
    mov x28, #0x1234
    adr x0, child
    mov x1, #5
    call 5
    expect 7
    mov x9, #0x1234
    cmp x28, x9
    b.ne .Lfail
    adr x0, child
    mov x1, #5
    call 5
    expect 7
    /* Faults and retired low aliases must never terminate the parent. */
    adr x21, fault_names
    mov x22, #9
.Lfaults:
    mov x0, x21
    mov x1, #7
    call 5
    expect -14
    add x21, x21, #7
    subs x22, x22, #1
    b.ne .Lfaults
    mov x0, #1
    adr x1, done
    mov x2, #done_end-done
    call 1
    expect done_end-done
    mov x0, #42
    call 0
.Lfail:
    mov x0, #1
    call 0
    b .Lfail
.size _start, .-_start
.section .rodata, "a"
greeting: .ascii "Phoenix system: init (type ok then Enter)\n"
greeting_end:
motd: .ascii "etc/motd"
note: .ascii "tmp/note"
full: .ascii "tmp/full"
file_names: .ascii "tmp/atmp/btmp/ctmp/dtmp/etmp/ftmp/g"
payload: .ascii "phoenix!"
child: .ascii "child"
fault_names: .ascii "fault-pfault-tfault-nfault-ifault-sfault-wfault-lfault-rfault-g"
done: .ascii "Phoenix system: file, console, child and fault checks passed\n"
done_end:
