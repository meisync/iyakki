"""Sample for the data science toolkit: count shapes and find motion with OpenCV.

Needs numpy and opencv-python-headless (the toolkit's Quick install item). No camera, no network, no model files:
it draws its own test picture, counts the coloured blobs, then compares two "frames" to see where something moved.
With a real camera the same two steps work on photos from the Camera tile (cv2.imread on the saved JPEGs).
Run:  python3 opencv_sample.py [OUT_DIR]
Writes: shapes.png (outlined blobs), motion.png (what changed between the frames), report.txt.
"""
import sys
from pathlib import Path

import cv2
import numpy as np

out = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
out.mkdir(parents=True, exist_ok=True)


def frame(dx):
    img = np.full((240, 320, 3), 235, np.uint8)
    cv2.circle(img, (70, 70), 30, (30, 30, 200), -1)
    cv2.rectangle(img, (160, 40), (230, 100), (30, 160, 30), -1)
    cv2.circle(img, (60 + dx, 180), 25, (200, 90, 30), -1)  # this one moves between frames
    return img


a, b = frame(0), frame(60)

# 1. count blobs: threshold what is not background, then outline each connected shape
gray = cv2.cvtColor(a, cv2.COLOR_BGR2GRAY)
_, mask = cv2.threshold(gray, 200, 255, cv2.THRESH_BINARY_INV)
contours, _ = cv2.findContours(mask, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
shapes = a.copy()
cv2.drawContours(shapes, contours, -1, (0, 0, 0), 2)
cv2.imwrite(str(out / "shapes.png"), shapes)

# 2. motion: absolute difference of the two frames, blurred and thresholded, then the box around the change
diff = cv2.absdiff(cv2.cvtColor(a, cv2.COLOR_BGR2GRAY), cv2.cvtColor(b, cv2.COLOR_BGR2GRAY))
diff = cv2.GaussianBlur(diff, (5, 5), 0)
_, moved = cv2.threshold(diff, 40, 255, cv2.THRESH_BINARY)
x, y, w, h = cv2.boundingRect(moved)
motion = b.copy()
cv2.rectangle(motion, (x, y), (x + w, y + h), (0, 0, 255), 2)
cv2.imwrite(str(out / "motion.png"), motion)

lines = [f"shapes found: {len(contours)}", f"changed region between the frames: x={x} y={y} width={w} height={h}", f"changed pixels: {int((moved > 0).sum())}"]
(out / "report.txt").write_text("\n".join(lines) + "\n")
print("\n".join(lines))
