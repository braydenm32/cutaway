# Models for auto-framing and background removal (Windows)

On Windows (and Linux development builds), Cutaway finds people with these
two small open models, run by Microsoft's ONNX Runtime (DirectML on the
graphics card). On a Mac it uses Apple's Vision framework instead, and these
aren't used. They're built into the program; nothing is downloaded.

| File | What it does | From | License |
|---|---|---|---|
| `face_detection_yunet_2023mar.onnx` | Finds faces (YuNet, 640×640 input) | [OpenCV Zoo](https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet) | MIT ([LICENSE-yunet.txt](LICENSE-yunet.txt)) |
| `human_segmentation_pphumanseg_2023mar.onnx` | Separates people from the background (PP-HumanSeg, 192×192) | [OpenCV Zoo](https://github.com/opencv/opencv_zoo/tree/main/models/human_segmentation_pphumanseg) | Apache 2.0 ([LICENSE-pphumanseg.txt](LICENSE-pphumanseg.txt)) |

SHA-256:

```
8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4  face_detection_yunet_2023mar.onnx
552d8a984054e59b5d773d24b9b12022b22046ceb2bbc4c9aaeaceb36a9ddf24  human_segmentation_pphumanseg_2023mar.onnx
```
