#!/usr/bin/env python3

# Polls 98x98 frames over serial and shows the two sensors side by side.
import pygame, sys, serial
import numpy as np

def shiftl(d: bytes, n: int) -> bytes:
    """Shift the data to the left n bits"""
    r = [(d[0] << n) & 0xff]
    for i in range(1, len(d)):
        r[-1] |= d[i]>>(8-n)
        r.append((d[i] << n) & 0xff)
    return bytes(r)

def shiftr(d: bytes, n: int) -> bytes:
    """Shift the data to the right n bits (n<8)"""
    r = [d[0] >> n, (d[0] << (8-n)) & 0xff]
    for i in range(1, len(d)):
        r[-1] |= d[i]>>n
        r.append((d[i] << (8-n)) & 0xff)
    return bytes(r)

serial_file = sys.argv[1] if len(sys.argv) >= 2 else "/dev/ttyACM1"
ser = serial.Serial(serial_file, timeout=1)

pygame.init()
screen = pygame.display.set_mode((1400, 700))
clock = pygame.time.Clock()
running = True

screen.fill("purple")
while running:
    for event in pygame.event.get():
        if event.type == pygame.QUIT:
            running = False

    ser.write(b"a")
    data = ser.read(9898)
    id, len0, len1 = data[-3:]
    length = len0 | (len1<<8)
    if length == 9898:
        data = shiftl(data, 4)
    elif length == 9897:
        data = shiftr(data, 3)
    else:
        print(f"weird length {length}")
        continue

    surface = pygame.image.frombytes(data[:98*98], (98, 98), "P")
    surface.set_palette([(x, x, x) for x in range(256)])
    surface = pygame.transform.flip(surface, not id, bool(id))
    surface = pygame.transform.scale(surface, (686, 686))
    screen.blit(surface, (700*id, 0))

    pygame.display.flip()
    print(clock.get_fps(), "FPS")

    clock.tick(60)

pygame.quit()
