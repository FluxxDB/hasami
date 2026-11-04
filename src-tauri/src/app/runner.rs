//! What runs each queued job: compress, convert and export with the bundled ffmpeg.

use crate::compress::{plan_compress, CompressDecision};
use crate::jobs::compress::compress;
use crate::jobs::convert::convert;
use crate::jobs::export::export;
use crate::jobs::queue::{JobKind, JobRunner, JobSpec};
use crate::jobs::{JobError, JobOutcome, JobProgress};
use crate::media::run::CancelToken;
use crate::media::{probe, FfmpegPaths};

use super::TOOLS_MISSING;

/// The one place that decides what runs a job.
pub(super) fn job_runner(paths: Option<FfmpegPaths>) -> Box<dyn JobRunner> {
    Box::new(MediaRunner { paths })
}

/// Runs compress, convert and export jobs with the bundled ffmpeg.
struct MediaRunner {
    paths: Option<FfmpegPaths>,
}

impl JobRunner for MediaRunner {
    fn run(
        &self,
        spec: &JobSpec,
        confirmed: bool,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(JobProgress),
    ) -> Result<JobOutcome, JobError> {
        let paths = self.paths.as_ref().ok_or_else(|| JobError::Message(TOOLS_MISSING.into()))?;
        match &spec.kind {
            JobKind::Compress => compress(paths, &spec.input, confirmed, cancel, progress),
            JobKind::Convert(target) => convert(paths, &spec.input, target.clone(), cancel, progress),
            JobKind::Export(job) => export(paths, &job.0, cancel, progress),
        }
    }

    /// Only compress knows its output size up front.
    fn preview(&self, spec: &JobSpec) -> Option<(u32, u32, f64)> {
        if spec.kind != JobKind::Compress {
            return None;
        }
        let info = probe(self.paths.as_ref()?, &spec.input).ok()?;
        match plan_compress(&info).ok()? {
            CompressDecision::Encode(plan) if plan.width > 0 => Some((plan.width, plan.height, plan.fps)),
            _ => None,
        }
    }
}
