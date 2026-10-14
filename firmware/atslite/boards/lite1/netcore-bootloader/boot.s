.syntax unified
.cpu cortex-m33
.thumb

.section .text.Reset
.global Reset
.type Reset, %function
Reset:
    // Load application address
    ldr r0, =0x01006000

    // Load stack pointer from app vector table
    ldr r1, [r0, #0]

    // Load reset vector from app vector table
    ldr r2, [r0, #4]

    // Set main stack pointer
    msr msp, r1

    // Jump to application
    bx r2

.size Reset, .-Reset
