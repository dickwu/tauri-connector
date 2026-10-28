use super::geometry::{RectCss, Viewport, mapping};

fn viewport() -> Viewport {
    Viewport {
        width_css: 800.0,
        height_css: 600.0,
        device_pixel_ratio: 2.0,
        visual_scale: 1.0,
        offset_left: 0.0,
        offset_top: 0.0,
    }
}

#[test]
fn measured_scale_is_used_for_one_two_and_fractional_dpr() {
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let mapped = mapping(
            &viewport(),
            (800.0 * scale) as u32,
            (600.0 * scale) as u32,
            None,
            None,
        )
        .unwrap();
        assert_eq!(mapped.css_to_image, [scale, 0.0, 0.0, scale, 0.0, 0.0]);
    }
}

#[test]
fn crop_clips_negative_origin_and_composes_resize() {
    let target = RectCss {
        x: -10.0,
        y: 10.0,
        width: 110.0,
        height: 100.0,
    };
    let mapped = mapping(&viewport(), 1600, 1200, Some(target), Some(100)).unwrap();
    assert_eq!(mapped.crop_px, [0, 20, 200, 200]);
    assert!(mapped.clipped);
    assert_eq!(mapped.css_to_image, [1.0, 0.0, 0.0, 1.0, 0.0, -10.0]);
}

#[test]
fn invalid_or_unproven_geometry_fails_closed() {
    let mut vp = viewport();
    vp.visual_scale = 1.25;
    assert!(mapping(&vp, 800, 600, None, None).is_err());
    assert!(mapping(&viewport(), 800, 700, None, None).is_err());
    assert!(mapping(&viewport(), 0, 600, None, None).is_err());
    assert!(
        mapping(
            &viewport(),
            800,
            600,
            Some(RectCss {
                x: f64::NAN,
                y: 0.0,
                width: 1.0,
                height: 1.0
            }),
            None
        )
        .is_err()
    );
    assert!(
        mapping(
            &viewport(),
            800,
            600,
            Some(RectCss {
                x: 900.0,
                y: 0.0,
                width: 1.0,
                height: 1.0
            }),
            None
        )
        .is_err()
    );
}

#[test]
fn oversized_native_dimensions_are_rejected_before_allocating() {
    assert!(super::check_dimensions(u32::MAX, u32::MAX).is_err());
    assert!(super::check_dimensions(4096, 4096).is_err());
    assert!(super::check_dimensions(4097, 4096).is_err());
}

#[tokio::test]
async fn callbacks_are_single_use_and_late_completion_is_dropped() {
    use std::sync::{Arc, Mutex};
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let callback = Arc::new(Mutex::new(super::CallbackSlot {
        sender: Some(sender),
        _permit: None,
    }));
    assert!(super::callback_pending(&callback));
    super::finish_callback(&callback, Ok(super::RawFrame::Encoded(vec![1])));
    super::finish_callback(&callback, Ok(super::RawFrame::Encoded(vec![2])));
    match receiver.await.unwrap().unwrap() {
        super::RawFrame::Encoded(bytes) => assert_eq!(bytes, [1]),
        _ => panic!("wrong frame"),
    }
    assert!(!super::callback_pending(&callback));
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let callback = Arc::new(Mutex::new(super::CallbackSlot {
        sender: Some(sender),
        _permit: None,
    }));
    drop(receiver);
    assert!(!super::callback_pending(&callback));
    super::finish_callback(&callback, Ok(super::RawFrame::Encoded(vec![3])));
    assert!(callback.lock().unwrap().sender.is_none());
}

#[tokio::test]
async fn cancelled_native_callback_keeps_its_slot_until_completion() {
    use std::sync::{Arc, Mutex};
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = slots.clone().acquire_owned().await.unwrap();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let callback = Arc::new(Mutex::new(super::CallbackSlot {
        sender: Some(sender),
        _permit: Some(permit),
    }));
    drop(receiver);
    assert!(!super::callback_pending(&callback));
    assert!(slots.clone().try_acquire_owned().is_err());
    super::finish_callback(&callback, Ok(super::RawFrame::Encoded(vec![1])));
    assert!(slots.try_acquire_owned().is_ok());
}

#[cfg(feature = "capture-codec")]
fn probe() -> super::Probe {
    super::Probe {
        viewport: Viewport {
            width_css: 4.0,
            height_css: 4.0,
            ..viewport()
        },
        sensitive_rects: vec![RectCss {
            x: -1.0,
            y: 1.0,
            width: 3.0,
            height: 2.0,
        }],
        uncovered_reasons: vec![],
        ready: true,
        document_stamp: 1.0,
        scroll_x: 0.0,
        scroll_y: 0.0,
        document_width: 4.0,
        document_height: 4.0,
    }
}

#[cfg(feature = "capture-codec")]
fn raw() -> super::RawFrame {
    super::RawFrame::Bitmap {
        bytes: [255, 23, 64, 255].repeat(16),
        width: 4,
        height: 4,
        stride: 16,
        order: super::PixelOrder::Rgba,
        premultiplied: false,
        opaque: false,
    }
}

#[test]
#[cfg(feature = "capture-codec")]
fn masks_are_applied_to_pixels_before_crop_resize_and_encoding() {
    let (png, _, width, height, _) = super::pixels::protect(
        raw(),
        &probe(),
        Some(RectCss {
            x: 0.0,
            y: 1.0,
            width: 4.0,
            height: 2.0,
        }),
        None,
    )
    .unwrap();
    assert_eq!((width, height), (4, 2));
    let image = image::load_from_memory(&png).unwrap().to_rgba8();
    for y in 0..2 {
        for x in 0..2 {
            assert_eq!(image.get_pixel(x, y).0, [0, 0, 0, 255]);
        }
    }
    assert_eq!(image.get_pixel(3, 0).0, [255, 23, 64, 255]);
    let (png, _, _, _, _) = super::pixels::protect(raw(), &probe(), None, Some(2)).unwrap();
    assert_eq!(image::load_from_memory(&png).unwrap().width(), 2);
}

#[test]
#[cfg(feature = "capture-codec")]
fn required_mask_refuses_unknown_coverage_and_malformed_frames() {
    let mut p = probe();
    p.uncovered_reasons.push("shadow_boundary".into());
    assert!(super::pixels::protect(raw(), &p, None, None).is_err());
    let mut p = probe();
    p.viewport.visual_scale = 2.0;
    assert!(super::pixels::protect(raw(), &p, None, None).is_err());
    assert!(super::pixels::decode(super::RawFrame::Encoded(vec![1, 2, 3, 4])).is_err());
    assert!(
        super::pixels::decode(super::RawFrame::Bitmap {
            bytes: vec![0; 8],
            width: 4,
            height: 4,
            stride: 16,
            order: super::PixelOrder::Rgba,
            premultiplied: false,
            opaque: false
        })
        .is_err()
    );
}

#[test]
#[cfg(feature = "capture-codec")]
fn native_bgra_premultiplication_and_stride_are_normalized() {
    let frame = super::RawFrame::Bitmap {
        bytes: vec![16, 32, 64, 128, 0, 0, 0, 0],
        width: 1,
        height: 1,
        stride: 8,
        order: super::PixelOrder::Bgra,
        premultiplied: true,
        opaque: false,
    };
    let image = super::pixels::decode(frame).unwrap();
    assert_eq!(image.get_pixel(0, 0).0, [128, 64, 32, 128]);
}

#[test]
fn pixel_budget_uses_the_specified_sixteen_million_pixel_boundary() {
    assert!(super::check_dimensions(4000, 4000).is_ok());
    assert!(super::check_dimensions(4001, 4000).is_err());
}

#[test]
fn stage_timings_use_elapsed_monotonic_time_and_do_not_invent_skipped_durations() {
    let measured =
        super::elapsed_stage(std::time::Instant::now() - std::time::Duration::from_millis(2));
    assert_eq!(measured["status"], "measured");
    let nanos = measured["elapsedNs"].as_u64().unwrap();
    assert!(nanos >= 2_000_000);
    assert!((measured["elapsedMs"].as_f64().unwrap() - nanos as f64 / 1_000_000.0).abs() < 1e-9);
    let skipped = super::not_applicable_stage("protected_memory_only");
    assert_eq!(skipped["status"], "not_applicable");
    assert!(skipped.get("elapsedNs").is_none());
    assert!(skipped.get("elapsedMs").is_none());
}

#[test]
#[cfg(feature = "capture-codec")]
fn pixel_stages_measure_actual_work_before_safe_output_transcoding() {
    let started = std::time::Instant::now();
    let (png, _, _, _, timings) = super::pixels::protect(raw(), &probe(), None, Some(2)).unwrap();
    let total = started.elapsed().as_nanos();
    assert_eq!(timings["clock"], "monotonic");
    let mut measured = 0u128;
    for stage in ["decode", "mask", "cropResize", "encode"] {
        assert_eq!(timings[stage]["status"], "measured");
        let elapsed = timings[stage]["elapsedNs"].as_u64().unwrap();
        assert!(elapsed > 0, "{stage} must report its real measured work");
        measured += u128::from(elapsed);
    }
    assert!(measured <= total);
    let (unchanged, skipped) = super::pixels::transcode(png.clone(), "png", 80).unwrap();
    assert_eq!(unchanged, png);
    assert_eq!(skipped["status"], "not_applicable");
    assert!(skipped.get("elapsedNs").is_none());
    let (webp, conversion) = super::pixels::transcode(png, "webp", 80).unwrap();
    assert_eq!(
        image::guess_format(&webp).unwrap(),
        image::ImageFormat::WebP
    );
    assert_eq!(conversion["status"], "measured");
    for stage in ["decode", "encode"] {
        assert!(conversion[stage]["elapsedNs"].as_u64().unwrap() > 0);
    }
}

#[test]
fn macos_title_bar_band_is_cropped_from_full_size_content_frames() {
    // R2-8: tao reported inner == outer (1120x690) while the WKWebView
    // viewport was 1120x658 below a 32 px title bar.
    let inset = super::title_bar_inset((1120, 690), (1120, 658));
    if cfg!(target_os = "macos") {
        assert_eq!(inset, Some(32));
        assert_eq!(super::title_bar_inset((2241, 1380), (2240, 1316)), Some(64));
    } else {
        assert_eq!(inset, None);
    }
    assert_eq!(super::title_bar_inset((1120, 658), (1120, 658)), None);
    assert_eq!(super::title_bar_inset((1000, 690), (1120, 658)), None);
    assert_eq!(super::title_bar_inset((1120, 1200), (1120, 658)), None);
}

#[test]
fn dom_capture_errors_keep_the_page_capture_code() {
    assert_eq!(
        super::dom_capture_error(
            "execution_failed: capture_backend_unavailable: dom_rendering needs snapdom (requestId: x)"
        ),
        "capture_backend_unavailable: dom_rendering needs snapdom (requestId: x)"
    );
    assert_eq!(
        super::dom_capture_error("execution_failed: boom happened"),
        "execution_failed: boom happened"
    );
    assert_eq!(super::dom_capture_error("timeout"), "timeout");
}

#[test]
fn webview_snapshot_extent_follows_the_css_viewport() {
    use super::webview_snapshot_extent as extent;
    // Default title bar: the view extends under it, the CSS viewport doesn't.
    assert_eq!(
        extent((1200.0, 828.0), Some((1200.0, 800.0)), (1200.0, 800.0)),
        (1200.0, 800.0)
    );
    // Overlay title bar: page content spans the full view; the safe area
    // still excludes the 32pt traffic-light band.
    assert_eq!(
        extent((1200.0, 800.0), Some((1200.0, 768.0)), (1200.0, 800.0)),
        (1200.0, 800.0)
    );
    // No safe-area API: full bounds.
    assert_eq!(extent((900.0, 600.0), None, (900.0, 600.0)), (900.0, 600.0));
    // Neither matches (zoomed page): keep the previous safe-area choice and let
    // the pixel pipeline's geometry check decide.
    assert_eq!(
        extent((1200.0, 828.0), Some((1200.0, 800.0)), (960.0, 640.0)),
        (1200.0, 800.0)
    );
}

#[test]
fn wait_for_rides_out_a_reload_and_reports_the_last_error() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let calls = std::cell::Cell::new(0);
    let ok = rt.block_on(super::service::wait_for(
        std::time::Duration::from_millis(1000),
        std::time::Duration::from_millis(5),
        || {
            calls.set(calls.get() + 1);
            let n = calls.get();
            async move {
                if n < 3 {
                    Err(format!("reloading {n}"))
                } else {
                    Ok(n)
                }
            }
        },
    ));
    assert_eq!(ok, Ok(3));
    let err = rt.block_on(super::service::wait_for::<(), _, _>(
        std::time::Duration::from_millis(30),
        std::time::Duration::from_millis(5),
        || async { Err("runtime gone".to_string()) },
    ));
    assert_eq!(err, Err("runtime gone".to_string()));
}

#[test]
fn a_hidden_page_names_the_window_and_the_remedy() {
    let message = super::service::hidden_window_error(Some("hidden")).unwrap();
    assert!(message.contains("--bring-to-front"));
    assert!(message.contains("locked"));
    assert!(super::service::hidden_window_error(Some("visible")).is_none());
    assert!(super::service::hidden_window_error(None).is_none());
    assert!(message.contains("windows focus"));
    let no_frame = super::service::no_frame_error("capture_not_ready: render_frame_timeout");
    assert!(no_frame.starts_with("capture_not_ready: render_frame_timeout"));
    assert!(no_frame.contains("windows focus") && no_frame.contains("--bring-to-front"));
}

#[test]
fn capture_reports_why_its_source_was_selected() {
    use super::{CaptureSource as S, source_selection};
    let order = [S::WebviewNative, S::WindowNative, S::DomRendering];
    let auto = source_selection(S::Auto, &order, S::WebviewNative);
    assert_eq!(auto["requested"], "auto");
    assert_eq!(auto["selected"], "webview_native");
    assert_eq!(auto["order"][1], "window_native");
    assert!(auto["reason"].as_str().unwrap().contains("CSS viewport"));
    let fell_back = source_selection(S::Auto, &order, S::DomRendering);
    assert!(fell_back["reason"].as_str().unwrap().contains("fallbacks"));
    let explicit = source_selection(S::WindowNative, &[S::WindowNative], S::WindowNative);
    assert!(explicit["reason"].as_str().unwrap().contains("explicitly"));
}
