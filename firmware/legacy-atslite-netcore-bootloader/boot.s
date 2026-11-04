.syntax unified
.cpu cortex-m33
.thumb

.section .text.Reset
.global Reset
.type Reset, %function
Reset:
    // Jump to the app at 0x01006000 with the SP and reset vector from its vector table.
    ldr r0, =0x01006000

    ldr r1, [r0, #0]

    ldr r2, [r0, #4]

    msr msp, r1

    bx r2

.size Reset, .-Reset
