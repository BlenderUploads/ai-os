/* HALCYON kernel layout.
 *
 * GRUB hands control over in 32-bit protected mode with paging off, so the
 * bootstrap lives at its physical load address (1 MiB).  Everything after it is
 * linked for the higher half at 0xFFFFFFFF80000000 but *loaded* at its physical
 * address via AT(), which is the address GRUB's ELF64 loader honours (p_paddr).
 */

ENTRY(_start)

KERNEL_VMA = 0xFFFFFFFF80000000;

SECTIONS {
    . = 1M;
    __image_phys_start = .;

    /* ---- identity-mapped bootstrap (32-bit code, boot page tables) ---- */
    .boot : ALIGN(4K) {
        KEEP(*(.multiboot_header))
        *(.boot.text)
    }

    .bootbss (NOLOAD) : ALIGN(4K) {
        *(.boot.bss)
    }

    . = ALIGN(4K);
    __kernel_phys_start = .;

    /* ---- higher half ---- */
    . += KERNEL_VMA;

    .text : AT(ADDR(.text) - KERNEL_VMA) ALIGN(4K) {
        __text_start = .;
        *(.text .text.*)
        __text_end = .;
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_VMA) ALIGN(4K) {
        __rodata_start = .;
        *(.rodata .rodata.*)
        __rodata_end = .;
    }

    .data : AT(ADDR(.data) - KERNEL_VMA) ALIGN(4K) {
        __data_start = .;
        *(.data .data.*)
        *(.got .got.plt)
        __data_end = .;
    }

    .bss : AT(ADDR(.bss) - KERNEL_VMA) ALIGN(4K) {
        __bss_start = .;
        *(COMMON)
        *(.bss .bss.*)
        . = ALIGN(4K);
        __bss_end = .;
    }

    . = ALIGN(4K);
    __kernel_virt_end = .;
    __kernel_phys_end = . - KERNEL_VMA;

    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.note .note.*)
        *(.comment)
        *(.dynamic)
        *(.dynsym)
        *(.dynstr)
        *(.hash)
        *(.gnu.hash)
    }
}
