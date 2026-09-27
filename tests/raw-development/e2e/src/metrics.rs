//! Pixel metrics for the A6 preview/export/histogram parity checks.
//!
//! Both compared images are the engine's sRGB-encoded RGBA8 output under the
//! same color transform (`OutputTarget::CpuPixels`); the metrics therefore
//! measure only the pipeline difference (linear-downscale-then-adjust preview
//! versus adjust-then-downscale export), not a color conversion.

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChannelDiff {
    pub mean_abs: f64,
    pub p99_abs: f64,
    pub max_abs: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ParityMetrics {
    pub per_channel: [ChannelDiff; 3],
    pub hist_mean_abs_diff: f64,
    pub hist_intersection_min: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Histogram {
    pub bins: usize,
    /// 3 channels x 256 bins, normalized counts (sum = 1 per channel).
    pub counts: Vec<f64>,
}

pub fn abs_diff_metrics(a: &[u8], b: &[u8]) -> [ChannelDiff; 3] {
    assert_eq!(a.len(), b.len(), "compared frames must have equal length");
    let pixels = a.len() / 4;
    let mut sums = [0u64; 3];
    let mut maxima = [0u32; 3];
    let mut hists = [[0u64; 256]; 3];
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for channel in 0..3 {
            let diff = (pa[channel] as i32 - pb[channel] as i32).unsigned_abs();
            sums[channel] += diff as u64;
            maxima[channel] = maxima[channel].max(diff);
            hists[channel][diff as usize] += 1;
        }
    }
    let mut channels = Vec::with_capacity(3);
    for channel in 0..3 {
        let mean = sums[channel] as f64 / pixels as f64;
        let mut p99 = 0u32;
        let mut seen = 0u64;
        let threshold = (pixels as f64 * 0.99).ceil() as u64;
        for (bin, count) in hists[channel].iter().enumerate() {
            seen += count;
            if seen >= threshold {
                p99 = bin as u32;
                break;
            }
        }
        channels.push(ChannelDiff {
            mean_abs: mean,
            p99_abs: p99 as f64,
            max_abs: maxima[channel],
        });
    }
    [channels.remove(0), channels.remove(0), channels.remove(0)]
}

pub fn histogram(rgba8: &[u8]) -> Histogram {
    let pixels = rgba8.len() / 4;
    let mut counts = [0u64; 256];
    for pixel in rgba8.chunks_exact(4) {
        // One defined luminance transform for histogram correspondence:
        // Rec.709 luma of the sRGB-encoded values, quantized to 256 bins.
        let luma = 0.2126 * pixel[0] as f64
            + 0.7152 * pixel[1] as f64
            + 0.0722 * pixel[2] as f64;
        counts[luma.round().clamp(0.0, 255.0) as usize] += 1;
    }
    Histogram {
        bins: 256,
        counts: counts.into_iter().map(|c| c as f64 / pixels as f64).collect(),
    }
}

pub fn histogram_metrics(a: &Histogram, b: &Histogram) -> (f64, f64) {
    assert_eq!(a.counts.len(), b.counts.len());
    let mean_abs = a
        .counts
        .iter()
        .zip(&b.counts)
        .map(|(x, y)| (x - y).abs())
        .sum::<f64>()
        / a.counts.len() as f64;
    let intersection = a
        .counts
        .iter()
        .zip(&b.counts)
        .map(|(x, y)| x.min(*y))
        .sum::<f64>();
    (mean_abs, intersection)
}

/// Histogram of each RGB channel independently (for per-channel parity).
pub fn histograms_per_channel(rgba8: &[u8]) -> Vec<Histogram> {
    let pixels = rgba8.len() / 4;
    let mut all = [[0u64; 256]; 3];
    for pixel in rgba8.chunks_exact(4) {
        for channel in 0..3 {
            all[channel][pixel[channel] as usize] += 1;
        }
    }
    all.into_iter()
        .map(|counts| Histogram {
            bins: 256,
            counts: counts.into_iter().map(|c| c as f64 / pixels as f64).collect(),
        })
        .collect()
}
