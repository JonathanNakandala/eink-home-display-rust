#include "image_decoder.h"
#include "runtime_image.h"
#include "decode_profile.h"
#include "esphome/core/log.h"
#include <algorithm>
#include <cmath>

namespace esphome::runtime_image {

static const char *const TAG = "image_decoder";

bool ImageDecoder::set_size(int width, int height) {
  bool success = this->image_->resize(width, height) > 0;
  this->size_valid_ = success;
  this->x_scale_ = static_cast<double>(this->image_->get_buffer_width()) / width;
  this->y_scale_ = static_cast<double>(this->image_->get_buffer_height()) / height;
  return success;
}

void ImageDecoder::draw_run(int x, int y, size_t count, int row_width, const Color &color) {
  RUNTIME_IMAGE_PROFILE_DO(decode_profile.draw_calls++);
  RUNTIME_IMAGE_PROFILE_SCOPE(draw_scope, decode_profile.draw_cycles);
#if RUNTIME_IMAGE_FILL_RUNS
  if (!this->size_valid_) {
    return;
  }
  if (this->x_scale_ == 1.0 && this->y_scale_ == 1.0) {
    this->image_->fill_run(x, y, count, row_width, color);
    return;
  }
#endif
  // Scaled, or filling is off: a call for each pixel, which is what a run was.
  for (size_t i = 0; i < count; i++) {
    this->draw(x, y, 1, 1, color);
    if (++x == row_width) {
      x = 0;
      y++;
    }
  }
}

void ImageDecoder::draw(int x, int y, int w, int h, const Color &color) {
  RUNTIME_IMAGE_PROFILE_DO(decode_profile.draw_calls++);
  RUNTIME_IMAGE_PROFILE_SCOPE(draw_scope, decode_profile.draw_cycles);
  if (!this->size_valid_) {
    return;
  }
  // The usual case, and the one that matters: the picture is drawn at its own size, so both scales are exactly 1 and the
  // pixels painted are the ones asked for. The general path below does its sums in double precision, which the ESP32-S3
  // (single precision hardware only) does in software, for every pixel. For a full-screen picture that was most of the time
  // spent decoding it. (A local change; see the README beside this file.)
  if (this->x_scale_ == 1.0 && this->y_scale_ == 1.0) {
    const int right = std::min(this->image_->get_buffer_width(), x + w);
    const int bottom = std::min(this->image_->get_buffer_height(), y + h);
    for (int i = x; i < right; i++) {
      for (int j = y; j < bottom; j++) {
        RUNTIME_IMAGE_PROFILE_SCOPE(pixel_scope, decode_profile.pixel_cycles);
        this->image_->draw_pixel(i, j, color);
      }
    }
    return;
  }
  auto width = std::min(this->image_->get_buffer_width(), static_cast<int>(std::ceil((x + w) * this->x_scale_)));
  auto height = std::min(this->image_->get_buffer_height(), static_cast<int>(std::ceil((y + h) * this->y_scale_)));
  for (int i = x * this->x_scale_; i < width; i++) {
    for (int j = y * this->y_scale_; j < height; j++) {
      this->image_->draw_pixel(i, j, color);
    }
  }
}

}  // namespace esphome::runtime_image
