# runtime_image, with four changes

A copy of ESPHome's `runtime_image` component (the decoders that `online_image` uses), taken from **ESPHome 2026.9.1**, with four
changes: three make a full-screen picture decode in a fraction of the time, and one logs how long the decoder takes. `reterminal-e1003.yaml`
loads it as a local external component, which takes the place of the built-in one.

## What is different

All four are marked `A local change` in the code. Nothing else is touched, and what is painted is the same except for the grey in (3), which is no worse.

1. **`image_decoder.cpp`, `ImageDecoder::draw`: no scaling when there is none.** The decoders paint one pixel at a time through
   `draw(x, y, 1, 1, colour)`, which did its sums in `double` (two `ceil`s and several multiplies) for every pixel. The ESP32-S3
   has hardware for single precision only, so each of those runs in software. When the picture is drawn at its own size the
   scales are exactly 1.0 and the sums change nothing, so that case now paints the pixels directly. For a 1872x1404 picture
   that is 2.6 million pixels.
2. **`qoi_decoder.cpp`: runs are stepped along, and the task watchdog is fed.** A QOI run was painted by dividing the pixel number
   by the width for every pixel (upstream has a `TODO` for it). It now keeps a column and a row. And one call to `decode` paints
   everything it was handed, where a byte can stand for up to 62 pixels, so a picture that is mostly runs could keep the loop
   task busy for longer than the task watchdog allows and reset the chip. The decoder now feeds the watchdog every 1024 chunks.
3. **`runtime_image.cpp`, `RuntimeImage::draw_pixel`: the grey is worked out in whole numbers.** The colour-to-grey sum used
   the doubles `0.2125`, `0.7154` and `0.0721`, three more software multiplies and two adds for every pixel. It now uses the weights
   54, 183 and 19 over 256. A grey pixel comes out as itself exactly (the doubles are a level short for 8 of the 256 greys, which
   was checked on a computer); any other colour is within one level of what they gave. Our pictures are all greys.
4. **`runtime_image.cpp`: the decoder is timed.** The microseconds spent inside `decode` are added up for each picture and logged
   with "Decoding complete", so a download's log says how much of it was the decoder and how much was waiting for the network.

## When ESPHome is upgraded

Compare this directory with the new version's, and take the upstream changes into the files above:

```
diff -ru .venv/Lib/site-packages/esphome/components/runtime_image esphome/components/runtime_image
```

If upstream has made the same fixes, delete this directory and the `external_components` entry in `reterminal-e1003.yaml`. Sending
them upstream (esphome/esphome) is the better home for them.
