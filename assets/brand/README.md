# Honorum brand assets

The Honorum symbol combines an open book and an hourglass.
The primary colors are brown `#8a5c3c` and ink `#2a221d`.
The reversed logo uses warm white `#fffdfa`.

## Source artwork

| File | Use |
| --- | --- |
| `honorum-logo.svg` | Horizontal logo on a light background |
| `honorum-logo-reversed.svg` | Horizontal logo on a dark background |
| `honorum-logo-stacked.svg` | Centered layouts |
| `honorum-logo-mono.svg` | One-color reproduction |
| `honorum-mark.svg` | Symbol without the wordmark |
| `honorum-icon.svg` | Square app icon source |
| `honorum-favicon.svg` | Symbol on a light background for browser tabs |

All source artwork uses vector paths. It needs no external font or image.
Keep the proportions and orientation. Leave clear space of at least one
quarter of the symbol height around the artwork.
Use the horizontal logo at widths of at least 160 pixels.
Use the symbol at smaller sizes.

## Client copies

- `apps/web/src/wordmark.js` contains the inline symbol and wordmark paths.
- `apps/web/public/` contains web icons and the cropped symbol.
- `apps/ios/Honorum/Assets.xcassets/BrandMark.imageset/` contains the iOS symbol.
- `apps/ios/Honorum/AppIcon.icon/` contains the Icon Composer app icon.
- Both font directories include an `OFL.txt` copy for distribution.

Update the client copies when the source artwork changes.
Keep each platform's icon format and appearance support.

## Font license

The wordmark uses Poppins Medium. The web and iOS apps also include that font.
Poppins uses the [SIL Open Font License 1.1](OFL-Poppins.txt).
Keep this notice with copies of the font.

| Font file | Format |
| --- | --- |
| `apps/web/public/fonts/poppins-500.woff2` | WOFF2 |
| `apps/ios/Honorum/Home/Poppins-Medium.ttf` | TrueType |

The font source is [The Poppins Project](https://github.com/itfoundry/Poppins).
