.syntax unified
.cpu cortex-m33
.thumb

.section .text.Reset
.global Reset
.type Reset, %function
Reset:
    // The netcore application's vector table: word 0 is the initial SP, word 1 the reset vector.
    ldr r0, =0x01006000

    ldr r1, [r0, #0]

    ldr r2, [r0, #4]

    msr msp, r1

    bx r2

.size Reset, .-Reset
