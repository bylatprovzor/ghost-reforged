//! Native launch requirements are checked before creating another game process.

#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    fn waveOutGetNumDevs() -> u32;
}

pub(crate) fn check() -> Result<(), String> {
    #[cfg(windows)]
    check_audio_outputs(unsafe { waveOutGetNumDevs() })?;
    Ok(())
}

#[cfg(any(windows, test))]
fn check_audio_outputs(count: u32) -> Result<(), String> {
    if count == 0 {
        // Zero also covers enumeration failure. A positive count is necessary,
        // but authentication and native API readiness still require confirmation.
        return Err(
            "Warcraft III has no available audio output. Check Windows Audio and install a persistent audio device for unattended hosting."
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_output_blocks_launch_without_claiming_that_muting_is_enough() {
        let error = check_audio_outputs(0).unwrap_err();
        assert!(error.contains("no available audio output"));
        assert!(error.contains("persistent audio device"));
        assert!(check_audio_outputs(1).is_ok());
        assert!(check_audio_outputs(2).is_ok());
    }
}
