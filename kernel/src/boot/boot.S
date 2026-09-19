/* HALCYON bootstrap: Multiboot2 header + 32-bit -> long mode transition.
 *
 * GRUB enters here in 32-bit protected mode, paging disabled, with:
 *     EAX = 0x36D76289 (multiboot2 magic)
 *     EBX = physical address of the multiboot information structure
 *
 * We validate the handoff, confirm the CPU has long mode, build boot page
 * tables (4 GiB identity + 1 GiB higher half), switch to 64-bit, and call
 * kmain() at its higher-half address.
 *
 * Written in AT&T syntax; see the `options(att_syntax)` on the global_asm! in
 * boot/mod.rs.
 */

/* global_asm! does not run the C preprocessor, so these are assembler symbols. */
.set MB2_MAGIC,      0xE85250D6
.set MB2_ARCH,       0                      /* i386 protected mode */
.set MB2_BOOT_MAGIC, 0x36D76289

/* ------------------------------------------------------------------ */
/* Multiboot2 header — must live in the first 32 KiB, 8-byte aligned.  */
/* ------------------------------------------------------------------ */
.section .multiboot_header, "a"
.align 8
mb_header_start:
    .long   MB2_MAGIC
    .long   MB2_ARCH
    .long   mb_header_end - mb_header_start
    .long   -(MB2_MAGIC + MB2_ARCH + (mb_header_end - mb_header_start))

    /* framebuffer request: 1024x768x32, flagged optional so a machine that
     * cannot provide it still boots far enough to tell us why. */
    .align 8
    .short  5                   /* type: framebuffer */
    .short  1                   /* flags: optional   */
    .long   20                  /* size              */
    .long   1024                /* width             */
    .long   768                 /* height            */
    .long   32                  /* depth             */

    /* modules must be page aligned (we map the initrd directly) */
    .align 8
    .short  6
    .short  0
    .long   8

    /* end tag */
    .align 8
    .short  0
    .short  0
    .long   8
mb_header_end:

/* ------------------------------------------------------------------ */
/* 32-bit bootstrap                                                    */
/* ------------------------------------------------------------------ */
.section .boot.text, "ax"
.code32
.global _start
.type _start, @function
_start:
    cli
    cld
    movl    $boot_stack_top, %esp

    /* Did a multiboot2 loader really put us here? */
    cmpl    $MB2_BOOT_MAGIC, %eax
    jne     err_not_multiboot

    /* Stash the multiboot info pointer where nothing below clobbers it. */
    movl    %ebx, %ebp

    /* --- CPUID available? (toggle EFLAGS.ID) --- */
    pushfl
    popl    %eax
    movl    %eax, %ecx
    xorl    $(1 << 21), %eax
    pushl   %eax
    popfl
    pushfl
    popl    %eax
    pushl   %ecx                /* restore original EFLAGS */
    popfl
    xorl    %ecx, %eax
    testl   $(1 << 21), %eax
    jz      err_no_cpuid

    /* --- extended leaves present? --- */
    movl    $0x80000000, %eax
    cpuid
    cmpl    $0x80000001, %eax
    jb      err_no_long_mode

    /* --- long mode supported? (leaf 0x80000001, EDX bit 29) --- */
    movl    $0x80000001, %eax
    cpuid
    testl   $(1 << 29), %edx
    jz      err_no_long_mode

    /* --- zero PML4 + both PDPTs (3 pages) --- */
    xorl    %eax, %eax
    movl    $boot_pml4, %edi
    movl    $(4096 * 3 / 4), %ecx
    rep stosl

    /* PML4[0]   -> low PDPT   (identity)     */
    movl    $boot_pdpt_low, %eax
    orl     $0x03, %eax                     /* present | writable */
    movl    %eax, boot_pml4

    /* PML4[511] -> high PDPT  (higher half)  */
    movl    $boot_pdpt_high, %eax
    orl     $0x03, %eax
    movl    %eax, boot_pml4 + 511 * 8

    /* low PDPT[0..3] -> the four page directories (4 GiB identity) */
    movl    $boot_pd, %eax
    orl     $0x03, %eax
    xorl    %ecx, %ecx
1:
    movl    %eax, boot_pdpt_low(, %ecx, 8)
    addl    $4096, %eax
    incl    %ecx
    cmpl    $4, %ecx
    jb      1b

    /* high PDPT[510] -> first page directory.
     * 0xFFFFFFFF80000000 >> 30 & 511 == 510, so this maps the kernel's
     * higher-half window onto physical 0..1 GiB. */
    movl    $boot_pd, %eax
    orl     $0x03, %eax
    movl    %eax, boot_pdpt_high + 510 * 8

    /* fill 2048 PD entries with 2 MiB pages covering 0..4 GiB */
    movl    $boot_pd, %edi
    movl    $0x83, %eax                     /* present | writable | huge */
    xorl    %ecx, %ecx
2:
    movl    %eax, (%edi)
    movl    $0, 4(%edi)
    addl    $0x200000, %eax
    addl    $8, %edi
    incl    %ecx
    cmpl    $2048, %ecx
    jb      2b

    /* --- turn on long mode --- */
    movl    $boot_pml4, %eax
    movl    %eax, %cr3

    movl    %cr4, %eax
    orl     $((1 << 5) | (1 << 7)), %eax    /* PAE | PGE */
    movl    %eax, %cr4

    movl    $0xC0000080, %ecx               /* IA32_EFER */
    rdmsr
    orl     $((1 << 8) | (1 << 11)), %eax   /* LME | NXE */
    wrmsr

    movl    %cr0, %eax
    orl     $((1 << 31) | (1 << 16) | 1), %eax  /* PG | WP | PE */
    movl    %eax, %cr0

    lgdt    boot_gdt_ptr
    ljmp    $0x08, $long_mode_entry

/* ------------------------------------------------------------------ */
/* 64-bit                                                              */
/* ------------------------------------------------------------------ */
.code64
long_mode_entry:
    movw    $0x10, %ax
    movw    %ax, %ds
    movw    %ax, %es
    movw    %ax, %ss
    movw    %ax, %fs
    movw    %ax, %gs

    movabsq $kernel_stack_top, %rsp

    /* Move the multiboot pointer out of %rbp before anything reuses it as a
     * frame pointer; %r12 survives the CR0/CR4 work below. */
    movl    %ebp, %r12d
    xorq    %rbp, %rbp

    /* Enable SSE: clear CR0.EM, set CR0.MP, set CR4.OSFXSR|OSXMMEXCPT.
     * The kernel is built soft-float, but the FPU must be in a sane state
     * before anything touches it. */
    movq    %cr0, %rax
    andq    $~(1 << 2), %rax
    orq     $(1 << 1), %rax
    movq    %rax, %cr0
    movq    %cr4, %rax
    orq     $(3 << 9), %rax
    movq    %rax, %cr4

    /* kmain(mbi_phys: u64) */
    movl    %r12d, %edi
    movabsq $kmain, %rax
    callq   *%rax

halt_forever:
    cli
    hlt
    jmp     halt_forever

/* ------------------------------------------------------------------ */
/* Early failure paths (still 32-bit, no framebuffer yet)              */
/* ------------------------------------------------------------------ */
.code32
err_not_multiboot:
    movl    $msg_not_multiboot, %esi
    jmp     early_panic
err_no_cpuid:
    movl    $msg_no_cpuid, %esi
    jmp     early_panic
err_no_long_mode:
    movl    $msg_no_long_mode, %esi
    jmp     early_panic

/* Print %esi to COM1 and to the VGA text buffer, then stop.  One of the two
 * is usually visible; on UEFI neither may be, which is why this path also
 * leaves the message in a fixed location for a post-mortem. */
early_panic:
    movl    %esi, %ebx
    /* serial: assume firmware left COM1 usable, just push bytes */
    movw    $0x3F8, %dx
3:
    movzbl  (%ebx), %eax
    testb   %al, %al
    jz      4f
    outb    %al, %dx
    incl    %ebx
    jmp     3b
4:
    /* VGA text mode, white on red */
    movl    %esi, %ebx
    movl    $0xB8000, %edi
5:
    movzbl  (%ebx), %eax
    testb   %al, %al
    jz      6f
    movb    %al, (%edi)
    movb    $0x4F, 1(%edi)
    incl    %ebx
    addl    $2, %edi
    jmp     5b
6:
    cli
    hlt
    jmp     6b

msg_not_multiboot:
    .asciz "HALCYON: not loaded by a multiboot2 bootloader\r\n"
msg_no_cpuid:
    .asciz "HALCYON: CPUID unavailable; this CPU is far too old\r\n"
msg_no_long_mode:
    .asciz "HALCYON: CPU has no long mode; a 64-bit CPU is required\r\n"

/* ------------------------------------------------------------------ */
/* Boot GDT: null, 64-bit code, data                                   */
/* ------------------------------------------------------------------ */
.align 16
boot_gdt:
    .quad 0x0000000000000000
    .quad 0x00AF9A000000FFFF        /* code: present, ring 0, exec/read, L=1 */
    .quad 0x00CF92000000FFFF        /* data: present, ring 0, read/write     */
boot_gdt_end:
boot_gdt_ptr:
    .word boot_gdt_end - boot_gdt - 1
    .long boot_gdt

/* ------------------------------------------------------------------ */
/* Boot-time page tables and stack (identity-mapped, below 4 GiB)      */
/* ------------------------------------------------------------------ */
.section .boot.bss, "aw", @nobits
.align 4096
.global boot_pml4
boot_pml4:
    .skip 4096
boot_pdpt_low:
    .skip 4096
boot_pdpt_high:
    .skip 4096
boot_pd:
    .skip 4096 * 4
.align 16
boot_stack_bottom:
    .skip 16384
boot_stack_top:

/* The real kernel stack lives in the higher half. */
.section .bss, "aw", @nobits
.align 4096
.global kernel_stack_bottom
kernel_stack_bottom:
    .skip 65536
.global kernel_stack_top
kernel_stack_top:
