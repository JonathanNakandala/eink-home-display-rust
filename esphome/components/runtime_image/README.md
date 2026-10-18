# runtime_image, with six changes

A copy of ESPHome's `runtime_image` component (the decoders that `online_image` uses), taken from **ESPHome 2026.9.1**, with six
changes: four make a full-screen picture decode in a fraction of the time, and two say where the decoder's time goes. `reterminal-e1003.yaml`
loads it as a local external component, which takes the place of the built-in one.

## What is different

All six are marked `A local change` in the code. Nothing else is touched, and what is painted is the same except for the grey in (3), which is no worse.

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

5. **`qoi_decoder.cpp`, `runtime_image.cpp`: a run is filled in, not painted a pixel at a time.** Measured (6), 96% of the pixels of a
   dashboard are in QOI runs (65,000 runs covering 2.53 of the 2.63 million pixels), and each was a call to `draw` and one to `draw_pixel`.
   Those two calls were three quarters of the decoder's time. A run is now `ImageDecoder::draw_run` -> `RuntimeImage::fill_run`, which
   `memset`s a row of a grey picture at a time (the other kinds of picture go on pixel by pixel, as before). The grey of a colour is
   worked out by one function, `gray_of_`, which `draw_pixel` uses too, so the two cannot differ. 2926 ms in the decoder became 466 ms,
   and the download and decode together 4.7 s became 1.1 s. The picture is the same: its checksum (below) matched that of a reference
   decoder written for the check (a Python QOI decoder with the same grey sum, run on the server's file) for the old path and the new
   one. `RUNTIME_IMAGE_FILL_RUNS` set to 0 (in `decode_profile.h`) goes back to a call for each pixel.
6. **`decode_profile.h`: the decoder's time is split up, and the picture can be checksummed.** CPU cycle counts (a one-instruction read) are
   added up for the call into `draw`, the call into `draw_pixel`, and the rest (reading chunks, the colour table), and logged with
   "Decoding complete", with how many runs and other chunks the picture was made of and how large the pieces handed over by the network
   were. `RUNTIME_IMAGE_PROFILE` set to 0 leaves all of it out. A CRC-32 of the decoded grey buffer can be logged as well
   (`RUNTIME_IMAGE_CHECKSUM` set to 1): the same bytes through a reference decoder give the same number, so a change to how pixels are
   painted can be checked against the picture itself. It reads the whole picture and took 157 ms on the chip, so it is off by default.

## When ESPHome is upgraded

Compare this directory with the new version's, and take the upstream changes into the files above:

```
diff -ru .venv/Lib/site-packages/esphome/components/runtime_image esphome/components/runtime_image
```

If upstream has made the same fixes, delete this directory and the `external_components` entry in `reterminal-e1003.yaml`. Sending
them upstream (esphome/esphome) is the better home for them.
