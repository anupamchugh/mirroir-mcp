# YOLO Icon Detection

Text-only OCR misses non-text UI elements — buttons, toggles, tab bar icons, activity rings. mirroir-mcp integrates YOLO CoreML models to detect these elements and merge them with Vision OCR results.

## Recommended Model: OmniParser v2.0

We recommend [Microsoft OmniParser v2.0](https://huggingface.co/microsoft/OmniParser-v2.0), a YOLOv8 model trained on UI screenshots to detect interactive elements. It outputs a single `icon` class with bounding boxes for clickable regions.

### Install via Hugging Face

```bash
pip install huggingface_hub
huggingface-cli download microsoft/OmniParser-v2.0 \
  --include "icon_detect/*" \
  --local-dir ~/.mirroir-mcp/models/icon_detect_download
```

### Convert to CoreML

Use [coremltools](https://github.com/apple/coremltools) to convert the PyTorch weights, or [Ultralytics](https://docs.ultralytics.com/modes/export/) which has built-in CoreML export:

```bash
pip install ultralytics coremltools
```

```python
from ultralytics import YOLO

model = YOLO("~/.mirroir-mcp/models/icon_detect_download/icon_detect/model.pt")
model.export(format="coreml", imgsz=640)
```

This produces a `.mlpackage`. Compile it to `.mlmodelc`:

```bash
xcrun coremlcompiler compile model.mlpackage ~/.mirroir-mcp/models/icon_detect.mlmodelc
```

### Verify

Restart mirroir-mcp and check the debug log:

```bash
grep "YOLO" ~/.mirroir-mcp/debug.log
# Should show: OCR: auto-detected YOLO model, using Vision + YOLO
```

Or run `mirroir doctor` — it reports whether a YOLO model is loaded.

## Configuration

```json
// .mirroir-mcp/settings.json
{
  "ocrBackend": "auto",
  "yoloConfidenceThreshold": 0.3
}
```

| Setting | Default | Description |
|---------|---------|-------------|
| `ocrBackend` | `"auto"` | `"auto"` merges Vision + YOLO when a model is present. `"yolo"` for icons only, `"both"` to always merge |
| `yoloModelURL` | `""` | URL to download a `.mlmodel` on first use (auto-compiles) |
| `yoloModelPath` | `""` | Local path to a pre-compiled `.mlmodelc` directory (overrides auto-detect and download) |
| `yoloConfidenceThreshold` | `0.3` | Minimum confidence for detections (0.0–1.0). Lower catches more elements but increases false positives |

## How It Works

The `CoreMLElementDetector` runs `VNCoreMLRequest` on each screenshot. Detected bounding boxes become `TapPoint` elements with the class label as the `text` field (e.g. `"icon"`). These are merged with Vision OCR text elements by `CompositeTextRecognizer`, with overlap deduplication to avoid double-counting regions where both backends detect something.

The merged element list is what `describe_screen` returns — the AI sees both text and icons as tap targets.

## Model Compatibility

Any YOLO CoreML model that outputs bounding boxes with class labels works. The detector uses the standard `VNCoreMLRequest` pipeline, so the requirements are:

- CoreML format (`.mlmodelc` compiled directory)
- Object detection output type (bounding boxes + class labels + confidence scores)
- Input: image (any resolution — Vision handles resizing)

Single-class models (like OmniParser's `icon`) work well for UI detection. Multi-class models (e.g. detecting "button", "toggle", "text_field" separately) also work — each class label becomes the element's text.

## Alternative: AI Vision Mode

For most use cases, [AI vision mode](../README.md#ai-vision-mode-embacle) produces richer results than YOLO without any model management. It identifies elements semantically (cards, tabs, buttons, navigation structure) rather than just bounding boxes.

YOLO is best when:
- You need fully local detection with no API calls
- You want to detect specific UI element types (toggles, switches, activity rings)
- You're running in CI where API latency matters
