# Weather icons

[Meteocons](https://github.com/basmilius/meteocons) by Bas Milius, the **monochrome** set from `@meteocons/svg-static` 0.1.0 (npm; the
package's sha1 is `8edf2246ef51b9be5994e446b920d32b35c1878d`), unchanged: MIT, with the author's own [LICENSE](LICENSE) beside them.
They draw in `currentColor`, so the page's text colour is theirs.

| Used for (`WeatherCondition`) | File |
|---|---|
| `clear` | `clear-day.svg` |
| `clouds` | `cloudy.svg` |
| `drizzle` | `drizzle.svg` |
| `rain` | `rain.svg` |
| `thunderstorm` | `thunderstorms.svg` |
| `snow` | `snow.svg` |

They are compiled into the server (`ICONS` in `chrome_render_display_image_generator.rs`), so the program carries their licence notice
with it (`THIRD_PARTY_LICENSES.txt` in a release). To use another: put the SVG here, and give it a line in `ICONS`.
