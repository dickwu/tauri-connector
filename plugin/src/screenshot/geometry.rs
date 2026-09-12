use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RectCss {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    pub width_css: f64,
    pub height_css: f64,
    pub device_pixel_ratio: f64,
    pub visual_scale: f64,
    pub offset_left: f64,
    pub offset_top: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Geometry {
    pub status: &'static str,
    pub css_to_image: [f64; 6],
    pub coordinate_space: &'static str,
    pub clipped: bool,
    pub crop_px: [u32; 4],
}

#[cfg(any(test, feature = "capture-codec"))]
pub(super) fn mapping(
    viewport: &Viewport,
    width: u32,
    height: u32,
    target: Option<RectCss>,
    max_width: Option<u32>,
) -> Result<Geometry, String> {
    let valid = [
        viewport.width_css,
        viewport.height_css,
        viewport.device_pixel_ratio,
    ]
    .iter()
    .all(|v| v.is_finite() && *v > 0.0);
    if !valid
        || width == 0
        || height == 0
        || max_width == Some(0)
        || viewport.visual_scale != 1.0
        || viewport.offset_left != 0.0
        || viewport.offset_top != 0.0
    {
        return Err(format!(
            "geometry_unknown: invalid viewport or unsupported visual zoom/offset (image={width}x{height}, viewport={}x{}, visualScale={}, visualOffset={},{} )",
            viewport.width_css,
            viewport.height_css,
            viewport.visual_scale,
            viewport.offset_left,
            viewport.offset_top
        ));
    }
    let sx = f64::from(width) / viewport.width_css;
    let sy = f64::from(height) / viewport.height_css;
    // A native viewport is uniformly scaled. At most two encoded pixels of
    // rounding are allowed; a title bar or full-document snapshot is not one.
    if (sx - sy).abs() * viewport.width_css.min(viewport.height_css) > 2.0 {
        return Err(format!(
            "geometry_unknown: capture extent {width}x{height} does not match viewport {}x{} (scaleX={sx},scaleY={sy})",
            viewport.width_css, viewport.height_css
        ));
    }
    let (crop_px, clipped) = if let Some(rect) = target {
        let (crop, clipped) = rect_pixels(rect, sx, sy, width, height)?;
        if crop[2] == 0 || crop[3] == 0 {
            return Err("target_outside_viewport".into());
        }
        (crop, clipped)
    } else {
        ([0, 0, width, height], false)
    };
    let resize = max_width
        .filter(|max| *max < crop_px[2])
        .map(|max| f64::from(max) / f64::from(crop_px[2]))
        .unwrap_or(1.0);
    // Output height is rounded during encoding, so retain its actual ratio.
    let resized_height = (f64::from(crop_px[3]) * resize).round().max(1.0);
    let resize_y = resized_height / f64::from(crop_px[3]);
    Ok(Geometry {
        status: "known",
        css_to_image: [
            sx * resize,
            0.0,
            0.0,
            sy * resize_y,
            -f64::from(crop_px[0]) * resize,
            -f64::from(crop_px[1]) * resize_y,
        ],
        coordinate_space: "layout_viewport_css",
        clipped,
        crop_px,
    })
}

#[cfg(any(test, feature = "capture-codec"))]
pub(super) fn rect_pixels(
    rect: RectCss,
    sx: f64,
    sy: f64,
    width: u32,
    height: u32,
) -> Result<([u32; 4], bool), String> {
    if ![rect.x, rect.y, rect.width, rect.height]
        .iter()
        .all(|v| v.is_finite())
        || rect.width < 0.0
        || rect.height < 0.0
        || !(rect.x + rect.width).is_finite()
        || !(rect.y + rect.height).is_finite()
    {
        return Err("geometry_unknown: invalid rectangle".into());
    }
    let raw = [
        (rect.x * sx).floor(),
        (rect.y * sy).floor(),
        ((rect.x + rect.width) * sx).ceil(),
        ((rect.y + rect.height) * sy).ceil(),
    ];
    let x1 = raw[0].clamp(0.0, f64::from(width)) as u32;
    let y1 = raw[1].clamp(0.0, f64::from(height)) as u32;
    let x2 = raw[2].clamp(0.0, f64::from(width)) as u32;
    let y2 = raw[3].clamp(0.0, f64::from(height)) as u32;
    Ok((
        [x1, y1, x2.saturating_sub(x1), y2.saturating_sub(y1)],
        raw[0] < 0.0 || raw[1] < 0.0 || raw[2] > f64::from(width) || raw[3] > f64::from(height),
    ))
}
