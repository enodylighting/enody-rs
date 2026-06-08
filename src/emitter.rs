//! Emitter-level control and spectral data access.
//!
//! Emitters are individual LED or light-producing channels inside a source.
//! Local implementations use [`crate::emitter::Emitter`]; host applications use
//! `emitter::remote::RemoteEmitter` when the `remote` feature is enabled.

use core::ops::RangeInclusive;

use crate::{message::Flux, spectral::SpectralData, Error, Identifier};

/// Local trait for an individual emitter channel.
#[allow(clippy::result_large_err)]
pub trait Emitter: Send + Sync {
    /// Returns the stable emitter identifier.
    fn identifier(&self) -> Identifier;

    /// Returns the supported flux range for this emitter.
    fn flux_range(&self) -> RangeInclusive<Flux>;

    /// Sets the target flux for this emitter.
    fn set_flux(&self, target_flux: Flux) -> Result<Flux, Error>;

    /// Returns spectral data for the emitter at the requested target flux.
    fn spectral_data(&self, target_flux: Flux) -> SpectralData;
}

#[cfg(feature = "remote")]
/// Remote emitter handles.
pub mod remote {
    use crate::{
        message::{
            Command, CommandMessage, EmitterCommand, EmitterEvent, EmitterInfo, Event, Flux,
            SpectralDataCommand, SpectralDataEvent, SPECTRAL_SAMPLE_BATCH_SIZE,
        },
        runtime::remote::RemoteRuntime,
        spectral::{SpectralData, SpectralSample},
        Identifier,
    };
    use heapless::Vec as HeaplessVec;

    /// Emitter accessed through a [`RemoteRuntime`].
    ///
    /// Cloning the underlying runtime is cheap, so remote emitters can be
    /// moved independently of their parent source.
    pub struct RemoteEmitter {
        info: EmitterInfo,
        remote: RemoteRuntime,
    }

    impl RemoteEmitter {
        /// Creates a remote emitter from protocol metadata and a runtime.
        pub fn new(info: EmitterInfo, remote: RemoteRuntime) -> Self {
            Self { info, remote }
        }

        /// Returns the emitter identifier.
        pub fn identifier(&self) -> Identifier {
            self.info.identifier()
        }

        /// Set the flux on this emitter.
        pub async fn set_flux(&self, flux: Flux) -> Result<Flux, crate::Error> {
            let command = Command::Emitter(EmitterCommand::FluxSet(flux));
            let command_message = CommandMessage::root(command, Some(self.identifier()));
            let event_message = self.remote.execute_command(command_message).await?;

            match event_message.event {
                Event::Emitter(EmitterEvent::FluxSet(result_flux)) => Ok(result_flux),
                _ => Err(crate::Error::UnexpectedResponse),
            }
        }

        /// Retrieve the full spectral data from the remote emitter.
        ///
        /// Sends Info, SampleCount, then batched SampleBatch commands to collect
        /// all spectral samples from the device.
        pub async fn spectral_data(&self) -> Result<SpectralData, crate::Error> {
            let emitter_id = self.identifier();

            // 1. Query sample count
            let command = Command::Emitter(EmitterCommand::SpectralData(
                SpectralDataCommand::SampleCount,
            ));
            let command_message = CommandMessage::root(command, Some(emitter_id));
            let event_message = self.remote.execute_command(command_message).await?;

            let Event::Emitter(EmitterEvent::SpectralData(SpectralDataEvent::SampleCount(count))) =
                event_message.event
            else {
                return Err(crate::Error::UnexpectedResponse);
            };

            // 2. Collect samples in batches
            let mut samples: HeaplessVec<SpectralSample, 401> = HeaplessVec::new();
            let mut offset = 0u32;

            while offset < count {
                let end = count.min(offset + SPECTRAL_SAMPLE_BATCH_SIZE as u32);
                let command = Command::Emitter(EmitterCommand::SpectralData(
                    SpectralDataCommand::SampleBatch(offset, end),
                ));
                let command_message = CommandMessage::root(command, Some(emitter_id));
                let event_message = self.remote.execute_command(command_message).await?;

                let Event::Emitter(EmitterEvent::SpectralData(SpectralDataEvent::SampleBatch(
                    sample_batch,
                ))) = event_message.event
                else {
                    return Err(crate::Error::UnexpectedResponse);
                };

                for sample in sample_batch.iter() {
                    samples
                        .push(sample.clone())
                        .map_err(|_| crate::Error::InsufficientData)?;
                }
                offset = end;
            }

            Ok(SpectralData::new(samples))
        }
    }
}
