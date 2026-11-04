use std::sync::Arc;

use iui::UI;
use iui::controls::{Area, AreaDrawParams, AreaHandler};
use parking_lot::Mutex;

use crate::mot_runner::MotRunner;
use crate::{CloneButShorter as _, tracking_canvas_helpers};

pub struct RunCanvas {
    pub ctx: UI,
    pub runner: Arc<Mutex<MotRunner>>,
}

impl std::fmt::Debug for RunCanvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunCanvas").finish_non_exhaustive()
    }
}

impl AreaHandler for RunCanvas {
    fn draw(&mut self, area: &Area, draw_params: &AreaDrawParams) {
        tracking_canvas_helpers::draw(self.ctx.c(), self.runner.c(), area, draw_params, false);
    }
}
