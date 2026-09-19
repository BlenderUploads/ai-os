/* HALCYON interrupt stubs.
 *
 * 256 uniform entry points, each exactly STUB_STRIDE (16) bytes, so the IDT
 * can be filled with `isr_stub_table + vector * 16` instead of 256 labels.
 *
 * Each stub normalises the stack: vectors that the CPU pushes an error code
 * for are left alone, the rest get a synthetic zero, so the common path always
 * sees the same frame.
 *
 * The dispatcher returns the trap frame to restore, which is how pre-emptive
 * task switching works here: the scheduler simply hands back a different
 * thread's frame.
 */

.section .text
.code64

.global isr_common
isr_common:
    cld

    /* Pushed last -> lowest address, so this order matches TrapFrame. */
    pushq %r15
    pushq %r14
    pushq %r13
    pushq %r12
    pushq %r11
    pushq %r10
    pushq %r9
    pushq %r8
    pushq %rbp
    pushq %rdi
    pushq %rsi
    pushq %rdx
    pushq %rcx
    pushq %rbx
    pushq %rax

    movq %rsp, %rdi
    call interrupt_dispatch
    movq %rax, %rsp          /* the frame to return to may not be the one we came in on */

    popq %rax
    popq %rbx
    popq %rcx
    popq %rdx
    popq %rsi
    popq %rdi
    popq %rbp
    popq %r8
    popq %r9
    popq %r10
    popq %r11
    popq %r12
    popq %r13
    popq %r14
    popq %r15

    addq $16, %rsp           /* discard vector and error code */
    iretq

/* ------------------------------------------------------------------ */
.balign 16
.global isr_stub_table
isr_stub_table:
.set vector, 0
.rept 256
    .balign 16
    /* These vectors arrive with a CPU-supplied error code; everything else
     * needs a placeholder so the frame layout is uniform. */
    .if (vector != 8) && (vector != 10) && (vector != 11) && (vector != 12) && (vector != 13) && (vector != 14) && (vector != 17) && (vector != 21) && (vector != 29) && (vector != 30)
        pushq $0
    .endif
    pushq $vector
    jmp isr_common
    .set vector, vector + 1
.endr
.balign 16
.global isr_stub_table_end
isr_stub_table_end:

/* The IDT is filled with isr_stub_table + vector * STUB_STRIDE, so a stub that
 * outgrew its slot would silently point every later vector at the wrong code.
 * idt::init() checks the span of this table against that assumption. */
