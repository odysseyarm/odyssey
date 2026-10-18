use std::sync::Arc;

use iui::UI;
use iui::controls::{Area, AreaDrawParams, AreaHandler};
use parking_lot::Mutex;

use crate::mot_runner::MotRunner;
use crate::{CloneButShorter, tracking_canvas_helpers};

pub struct RunRawCanvas {
    pub ctx: UI,
    pub runner: Arc<Mutex<MotRunner>>,
}

impl AreaHandler for RunRawCanvas {
    fn draw(&mut self, _area: &Area, draw_params: &AreaDrawParams) {
        tracking_canvas_helpers::draw(self.ctx.c(), self.runner.c(), _area, draw_params, true);
    }
}
