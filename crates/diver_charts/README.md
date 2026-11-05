# diver_charts

Charts for DataDiver, and later for a Python package.

A chart is a plain description of its data and options (`BarChart`).
`layout(width, height, &theme, &measure)` turns it into a `Scene`: rectangles,
lines and text in device-independent pixels, drawn in order, plus the marks
the pointer can land on, each with its tooltip. The crate draws nothing and
has no dependencies. A backend draws the scene and supplies text widths
through `Measure`. Today that backend is Direct2D in the gallery example.

```rust
use diver_charts::{BarChart, Theme};

let chart = BarChart::new(["Q1", "Q2", "Q3", "Q4"])
    .series("North", [420.0, 465.0, 510.0, 590.0])
    .series("South", [290.0, 340.0, 365.0, 371.0])
    .title("Sales by region");
let scene = chart.layout(640.0, 400.0, &Theme::light(), &measure);
```

## What's there

- `bar`: columns or horizontal bars, one series or several, grouped or
  stacked. Handles negative values, missing values, value labels and rotated
  category names. Series past the palette's eighth fold into "Other".
- `scale`: round tick values, and linear and band scales.
- `format`: numbers for ticks, labels and tooltips (1,284 / 12.9K / 4.2M).
- `scene`: the shapes, the marks, and the tooltip layout.
- `theme`: light and dark colors, fonts and mark sizes.

## The gallery

```powershell
cargo run -p diver_charts --example gallery
```

The overview shows every example as a card, and the sidebar opens each one on
its own. Hover a bar to see its tooltip. Up and Down move between examples,
Home goes back to the overview, and T switches between light and dark.

Save PNG, at the top of an open example or on a hovered card (Ctrl+S on an
open example), asks where to save the chart. The image is drawn at the size the
chart is shown, with twice the pixels so it stays sharp.

`--capture out.png` paints one frame to a PNG and exits. `--sample N` opens
example N first, `--dark` uses the dark theme, `--size 1400x900` sets the
size, and `--hover X,Y` points there before the picture is taken. `--save
chart.png` saves the open example as the button does, without the dialog.
