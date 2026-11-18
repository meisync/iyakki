# Data science samples

Two small scripts for the planned data science toolkit (see docs/development/DATA-SCIENCE-ADDON-PLAN.md). The toolkit is **not built yet**: IYAKKI has no Python with scikit-learn or OpenCV on the device today, so these cannot run there yet.

| Script | What it does | Needs |
| --- | --- | --- |
| sklearn_sample.py | makes a 600-row labelled test set (CSV), trains a random forest, writes accuracy and a confusion matrix | numpy, scikit-learn |
| opencv_sample.py | draws its own picture, counts the shapes, finds where something moved between two frames | numpy, opencv-python-headless |

Both need no network, no camera and no model files. They were run on a PC in a throwaway virtual environment (scikit-learn 1.9.1, OpenCV 5.0.0, NumPy 2.5.4): the first gave accuracy 0.953, the second found 3 shapes and the moved region. Run them with `python3 -I SCRIPT [OUT_DIR]`.

When the toolkit exists these become its first examples for the agent's `python_run` tool and a base for its install self-test.

The Flows app has two sample flows for these scripts (Scikit-learn: make a test set and train a model; OpenCV: count shapes and find motion). Each starts with a check that says plainly the toolkit is not installed yet when Python cannot import the module.
