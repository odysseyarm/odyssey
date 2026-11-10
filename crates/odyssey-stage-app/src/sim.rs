//! The connection to a running hub's simulator, and what it was last told.

use std::sync::Arc;

use odyssey_hub_server::sim::{Control, ControlError};
use odyssey_sim::dataset::SceneScreen;
use odyssey_sim::{Command, Snapshot, Target, Uuid};
use odyssey_stage::{Compiled, SimFeed};

#[derive(Default)]
pub(crate) struct SimLink {
    control: Option<Control>,
    feed: SimFeed,
    /// The stage the simulator's screens and devices were last set up for.
    set_up_for: Option<Arc<Compiled>>,
    /// Why the hub's tracker may disagree with the staged display.
    pub(crate) note: Option<String>,
}

impl SimLink {
    async fn open(&mut self) -> Result<Control, ControlError> {
        match self.control.take() {
            Some(control) => Ok(control),
            None => {
                self.set_up_for = None;
                Control::connect().await
            }
        }
    }

    /// Keep the connection unless it is what failed.
    fn close<T>(&mut self, control: Control, result: Result<T, ControlError>) -> Result<T, String> {
        if matches!(result, Ok(_) | Err(ControlError::Model(_))) {
            self.control = Some(control);
        }
        result.map_err(|error| format!("hub simulator: {error}"))
    }

    /// The simulated unit and the dongle it belongs to.
    pub(crate) fn target(&self, unit: Uuid) -> Target {
        self.feed.target(unit)
    }

    pub(crate) async fn execute(
        &mut self,
        target: Target,
        command: Command,
    ) -> Result<Snapshot, String> {
        let mut control = self.open().await.map_err(|error| error.to_string())?;
        let result = async {
            control.execute(target, command).await?;
            control.snapshot().await
        }
        .await;
        self.close(control, result)
    }

    /// The hub's screen calibrations: what its tracker believes.
    pub(crate) async fn hub_screens(&mut self) -> Result<Vec<SceneScreen>, String> {
        let mut control = self.open().await.map_err(|error| error.to_string())?;
        let result = control.screens().await;
        self.close(control, result)
    }

    /// Bring the hub's simulator to the stage at `t_s`. A stage that binds
    /// nothing to the hub leaves the simulator alone.
    pub(crate) async fn drive(&mut self, compiled: &Arc<Compiled>, t_s: f64) -> Result<(), String> {
        let bound = compiled.stage().hub_devices().next().is_some();
        if !bound && self.set_up_for.is_none() {
            return Ok(());
        }
        let mut control = self.open().await.map_err(|error| error.to_string())?;
        let result = self.run(&mut control, compiled, bound, t_s).await;
        self.close(control, result)
    }

    async fn run(
        &mut self,
        control: &mut Control,
        compiled: &Arc<Compiled>,
        bound: bool,
        t_s: f64,
    ) -> Result<(), ControlError> {
        if !bound {
            self.set_up_for = None;
            self.note = None;
            return control.follow_screens().await;
        }
        let stage = compiled.stage();
        let same_stage = self
            .set_up_for
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(previous, compiled));
        let same_display = self.set_up_for.as_ref().is_some_and(|previous| {
            previous.screen().calibration == compiled.screen().calibration
                && previous.stage().tracker_knows_surface == stage.tracker_knows_surface
                && previous.stage().display.is_hub_screen() == stage.display.is_hub_screen()
        });
        if !same_display {
            self.note = None;
            if stage.display.is_hub_screen() {
                control.follow_screens().await?;
            } else {
                let screen = compiled.screen().clone();
                let mut believed = screen.clone();
                if !stage.tracker_knows_surface {
                    believed.calibration.surface = None;
                }
                control
                    .execute(
                        Target::default(),
                        Command::Screens {
                            screens: vec![screen],
                        },
                    )
                    .await?;
                match control.calibrate(vec![believed]).await {
                    Ok(()) => {}
                    Err(ControlError::Model(reason)) => self.note = Some(reason),
                    Err(error) => return Err(error),
                }
            }
        }
        if !same_stage {
            self.feed = SimFeed::default();
            self.set_up_for = Some(Arc::clone(compiled));
        }
        let snapshot = control.snapshot().await?;
        let commands = self
            .feed
            .step(compiled, &snapshot, t_s)
            .map_err(|error| ControlError::Model(error.to_string()))?;
        for (target, command) in commands {
            control.execute(target, command).await?;
        }
        Ok(())
    }
}
