import sys
from PIL import Image
import serial

serial_file = sys.argv[1] if len(sys.argv) >= 2 else "/dev/ttyACM1"

ser = serial.Serial(serial_file, timeout=1)
ser.write(b"a")
data = ser.read(98*98 + 98*3)
print(f"read {len(data)} bytes")

open("data.bin", "wb").write(data)
