//! Camera ownership stays in the physical unit's handler, alongside its controls
//! and transport changes, rather than opening another USB handle from an RPC task.

use futures::StreamExt as _;
use futures::stream::BoxStream;
use odyssey_hub_common::device::{DeviceCapabilities, EventsTransport};
use odyssey_protocol::image::{ImageFrame, ImageStatusCode, ImageStreamConfig};
use odyssey_usb::image::ImageStream;
use tonic::Status;
use tracing::warn;

use super::DeviceHandler;
use crate::device_tasks::image_stream::{ImageDelivery, ImageResponseStream};

// Start can need both a status wait and cleanup Stop before reporting old
// firmware as unsupported. Keep that bounded handshake intact.
const IMAGE_SESSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

pub(super) struct ImageSession {
    stream: ImageStream,
    delivery: ImageDelivery,
    renewals: BoxStream<'static, ()>,
}

pub(super) async fn next_image_event(
    session: &mut Option<ImageSession>,
) -> Option<odyssey_usb::Result<ImageFrame>> {
    let Some(session) = session else {
        return std::future::pending().await;
    };
    loop {
        tokio::select! {
            biased;
            () = session.delivery.closed() => return None,
            renewal = session.renewals.next() => match renewal {
                // A closed stream already reports Closed through the frame path.
                Some(()) => { let _ = session.stream.renew_lease(); }
                None => return None,
            },
            frame = session.stream.next() => return frame,
        }
    }
}

fn image_error(error: odyssey_usb::Error) -> Status {
    match error {
        odyssey_usb::Error::ImageUsbRequired
        | odyssey_usb::Error::ImageStatus(ImageStatusCode::UsbRequired) => {
            Status::failed_precondition("image capture requires USB events transport")
        }
        odyssey_usb::Error::ImageUnsupported
        | odyssey_usb::Error::ImageStatus(ImageStatusCode::Unsupported) => {
            Status::unimplemented("device firmware does not support live images")
        }
        odyssey_usb::Error::ImageStatus(ImageStatusCode::UnsupportedCamera) => {
            Status::invalid_argument("camera is not supported by this device")
        }
        odyssey_usb::Error::ImageStatus(ImageStatusCode::Busy) => {
            Status::resource_exhausted("device already has an active image session")
        }
        odyssey_usb::Error::ImageStatus(ImageStatusCode::RebootRequired) => {
            Status::failed_precondition(
                "Legacy image mode requires reboot/power-cycle and reconnect to restore tracking.",
            )
        }
        odyssey_usb::Error::ImageStatus(ImageStatusCode::LeaseExpired) => {
            Status::deadline_exceeded("image capture lease expired; device restored tracking")
        }
        odyssey_usb::Error::InvalidImage(reason) => Status::data_loss(reason),
        error => Status::unavailable(format!("image transport failed: {error}")),
    }
}

impl DeviceHandler {
    #[expect(
        clippy::result_large_err,
        reason = "image RPC replies carry tonic::Status; boxing would add an allocation"
    )]
    pub(super) async fn start_image_session(
        &mut self,
        config: ImageStreamConfig,
        renewals: BoxStream<'static, ()>,
    ) -> Result<ImageResponseStream, Status> {
        if self
            .image_session
            .as_ref()
            .is_some_and(|session| session.delivery.is_closed())
        {
            self.stop_image_session(None).await;
        }
        if self.image_session.is_some() {
            return Err(Status::resource_exhausted(
                "device already has an active image session",
            ));
        }
        if self.image_reboot_required {
            return Err(Status::failed_precondition(
                "Legacy image mode requires reboot/power-cycle and reconnect.",
            ));
        }
        if !(1..=15).contains(&config.fps) {
            return Err(Status::invalid_argument(
                "image FPS must be between 1 and 15",
            ));
        }
        if self.device.events_transport != EventsTransport::Wired
            || !self.device.events_connected
            || !self
                .device
                .capabilities
                .contains(DeviceCapabilities::EVENTS)
            || !self
                .device
                .capabilities
                .contains(DeviceCapabilities::CONTROL)
        {
            return Err(Status::failed_precondition(
                "image capture requires connected USB events transport",
            ));
        }
        // Control and event endpoints can differ. Only the active wired events link
        // owns image packets. Never select USB control merely because it exists.
        let device = self
            .connections
            .events_device
            .clone()
            .ok_or_else(|| Status::failed_precondition("USB events endpoint is unavailable"))?;
        // Firmware owns marker/image interleaving. Keep IMU, impact and battery
        // subscriptions alive rather than disabling every sensor for diagnostics.
        if self.stream_timers.paused {
            self.on_streams_resume_due().await;
        }
        let result =
            tokio::time::timeout(IMAGE_SESSION_TIMEOUT, device.stream_images(config)).await;
        let stream = match result {
            Ok(Ok(stream)) => stream,
            result => {
                self.image_reboot_required = matches!(self.device.product_id, 0x520f | 0x5210)
                    && !matches!(
                        &result,
                        Ok(Err(odyssey_usb::Error::ImageUnsupported
                            | odyssey_usb::Error::ImageUsbRequired))
                    );
                if !self.image_reboot_required {
                    self.on_streams_resume_due().await;
                }
                return Err(match result {
                    Ok(Err(error)) => image_error(error),
                    Err(_) => Status::deadline_exceeded("image start timed out"),
                    Ok(Ok(_)) => unreachable!(),
                });
            }
        };
        self.image_reboot_required = matches!(self.device.product_id, 0x520f | 0x5210);
        let (delivery, response) = ImageDelivery::channel();
        self.image_session = Some(ImageSession {
            stream,
            delivery,
            renewals,
        });
        Ok(response)
    }

    pub(super) async fn on_image_event(&mut self, frame: Option<odyssey_usb::Result<ImageFrame>>) {
        match frame {
            Some(Ok(frame)) => {
                if let Some(session) = &self.image_session {
                    session.delivery.publish(frame.into());
                }
            }
            Some(Err(error)) => self.stop_image_session(Some(image_error(error))).await,
            None => self.stop_image_session(None).await,
        }
    }

    pub(super) async fn stop_image_session(&mut self, error: Option<Status>) {
        let Some(mut session) = self.image_session.take() else {
            return;
        };
        let stopped = tokio::time::timeout(IMAGE_SESSION_TIMEOUT, session.stream.stop()).await;
        if matches!(
            &stopped,
            Ok(Err(odyssey_usb::Error::ImageStatus(
                ImageStatusCode::RebootRequired
            )))
        ) {
            self.image_reboot_required = true;
        }
        let cleanup_error = match stopped {
            Ok(Ok(())) => None,
            Ok(Err(error)) => {
                warn!("Image stop failed for {:02x?}: {error}", self.device.uuid);
                Some(image_error(error))
            }
            Err(_) => {
                warn!("Image stop timed out for {:02x?}", self.device.uuid);
                Some(Status::deadline_exceeded("image stop timed out"))
            }
        };
        session.delivery.finish(error.or(cleanup_error));
        drop(session);
        // Reuse the handler's normal stream lifecycle, including standby links and
        // its retry machinery. This also runs after a rejected Start command.
        if !self.image_reboot_required {
            self.on_streams_resume_due().await;
        }
    }
}
