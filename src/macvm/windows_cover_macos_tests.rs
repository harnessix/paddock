//! The edge colors of a window picture, on macOS: a real
//! `NSBitmapImageRep` and its `CGImage`, read the way the cover reads the
//! picture of a host window at the press of a resize.

use super::*;
use objc2_app_kit::NSDeviceRGBColorSpace;

/// A picture of `w` by `h` pixels with the color of each pixel from `at`.
fn picture(w: isize, h: isize, at: impl Fn(isize, isize) -> [f64; 3]) -> CFRetained<CGImage> {
    use objc2_core_foundation::CFData;
    use objc2_core_graphics::{
        CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImageAlphaInfo,
    };

    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let color = at(x, y);
            pixels.extend(color.map(|c| (c * 255.0).round() as u8));
            pixels.push(255);
        }
    }
    // SAFETY: the byte slice is valid for this call and CFData copies it.
    let data = unsafe { CFData::new(None, pixels.as_ptr(), pixels.len() as isize) }.unwrap();
    let provider = CGDataProvider::with_cf_data(Some(&data)).unwrap();
    let space = CGColorSpace::new_device_rgb().unwrap();
    // SAFETY: the provider owns the bytes, the dimensions match, and the decode array is null.
    unsafe {
        CGImage::new(
            w as usize,
            h as usize,
            8,
            32,
            w as usize * 4,
            Some(&space),
            CGBitmapInfo(CGImageAlphaInfo::Last.0),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .expect("an RGB image")
}

fn near(a: Rgba, b: [f64; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 0.06)
}

#[test]
fn the_edge_colors_come_from_the_title_bar_the_side_and_the_bottom() {
    // A 200 x 100 point window at scale 2: a gray title bar of 28 points,
    // a white page, a black status bar of the last 10 points, and a line
    // of blue text that touches the right edge in a few rows.
    let gray = [0.6, 0.6, 0.6];
    let white = [1.0, 1.0, 1.0];
    let black = [0.0, 0.0, 0.0];
    let blue = [0.0, 0.0, 1.0];
    let img = picture(400, 200, |x, y| {
        if y < 56 {
            gray
        } else if y >= 180 {
            black
        } else if (100..104).contains(&y) && x > 300 {
            blue
        } else {
            white
        }
    });
    let e = edges_of(&img, (200.0, 100.0), 28.0, 10.0).expect("edge colors");
    assert!(near(e.title, gray), "{:?}", e.title);
    assert!(near(e.side, white), "{:?}", e.side);
    assert!(near(e.bottom, black), "{:?}", e.bottom);
}

#[test]
fn a_clear_picture_gives_no_edge_colors() {
    // SAFETY: as above.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            40,
            40,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            32,
        )
    }
    .expect("a bitmap");
    let clear = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 0.0);
    for y in 0..40 {
        for x in 0..40 {
            rep.setColor_atX_y(&clear, x, y);
        }
    }
    let img = rep.CGImage().expect("a CGImage");
    assert!(edges_of(&img, (40.0, 40.0), 10.0, 4.0).is_none());
}
