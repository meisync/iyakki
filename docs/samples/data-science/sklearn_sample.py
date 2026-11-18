"""Sample for the data science toolkit: make a labelled test set with scikit-learn, train a model, write the CSV.

Needs numpy and scikit-learn (the toolkit's Quick install item). Reads nothing, needs no network.
Run:  python3 sklearn_sample.py [OUT_DIR]
Writes: test_set.csv (features f0..f5 + label), report.txt (accuracy, confusion matrix, feature weights).
"""
import csv
import sys
from pathlib import Path

import numpy as np
from sklearn.datasets import make_classification
from sklearn.ensemble import RandomForestClassifier
from sklearn.metrics import accuracy_score, confusion_matrix
from sklearn.model_selection import train_test_split

out = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
out.mkdir(parents=True, exist_ok=True)

# 600 rows, 6 numeric features (4 informative), 2 classes, a little label noise; the seed makes it repeatable.
X, y = make_classification(n_samples=600, n_features=6, n_informative=4, n_redundant=1, n_classes=2, flip_y=0.03, class_sep=1.2, random_state=7)

with open(out / "test_set.csv", "w", newline="") as f:
    w = csv.writer(f)
    w.writerow([f"f{i}" for i in range(X.shape[1])] + ["label"])
    for row, label in zip(X, y):
        w.writerow([f"{v:.4f}" for v in row] + [int(label)])

Xtr, Xte, ytr, yte = train_test_split(X, y, test_size=0.25, random_state=7, stratify=y)
model = RandomForestClassifier(n_estimators=120, random_state=7).fit(Xtr, ytr)
pred = model.predict(Xte)
lines = [
    f"rows {len(y)}, train {len(ytr)}, test {len(yte)}",
    f"accuracy {accuracy_score(yte, pred):.3f}",
    f"confusion matrix (rows = true, columns = predicted): {confusion_matrix(yte, pred).tolist()}",
    "feature importance: " + ", ".join(f"f{i}={v:.2f}" for i, v in enumerate(model.feature_importances_)),
]
(out / "report.txt").write_text("\n".join(lines) + "\n")
print("\n".join(lines))
