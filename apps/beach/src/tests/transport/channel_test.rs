#![recursion_limit = "1024"]

#[cfg(test)]
mod tests {
    use crate::transport::{
        Transport, TransportMode,
        channel::{ChannelOptions, ChannelPurpose, ChannelReliability},
        mock::MockTransport,
    };
    use anyhow::Result;

    #[test_timeout::tokio_timeout_test]
    async fn test_channel_creation() -> Result<()> {
        let transport = MockTransport::new();

        // Create control channel
        let control_channel = transport.channel(ChannelPurpose::Control).await?;
        assert_eq!(control_channel.purpose(), ChannelPurpose::Control);
        assert_eq!(control_channel.reliability(), ChannelReliability::Reliable);
        assert!(control_channel.is_open());

        // Create output channel
        let output_channel = transport.channel(ChannelPurpose::Output).await?;
        assert_eq!(output_channel.purpose(), ChannelPurpose::Output);
        assert!(output_channel.is_open());

        Ok(())
    }

    #[test_timeout::tokio_timeout_test]
    async fn test_channel_purpose_routing() -> Result<()> {
        let transport = MockTransport::new();

        // Create different purpose channels
        let control = transport.channel(ChannelPurpose::Control).await?;
        let output = transport.channel(ChannelPurpose::Output).await?;
        let custom = transport.channel(ChannelPurpose::Custom(1)).await?;

        // Verify they are different channels
        assert_eq!(control.purpose(), ChannelPurpose::Control);
        assert_eq!(output.purpose(), ChannelPurpose::Output);
        assert_eq!(custom.purpose(), ChannelPurpose::Custom(1));

        // Verify labels
        assert_eq!(control.label(), "mock-channel");
        assert_eq!(output.label(), "mock-channel");
        assert_eq!(custom.label(), "mock-channel");

        Ok(())
    }

    #[test_timeout::tokio_timeout_test]
    async fn test_multiple_channels() -> Result<()> {
        let transport = MockTransport::new();

        // Create multiple channels
        let _control = transport.channel(ChannelPurpose::Control).await?;
        let _output = transport.channel(ChannelPurpose::Output).await?;

        // Get same channel again should return existing
        let control2 = transport.channel(ChannelPurpose::Control).await?;
        assert_eq!(control2.purpose(), ChannelPurpose::Control);

        Ok(())
    }

    #[test_timeout::tokio_timeout_test]
    async fn test_channel_send_receive() -> Result<()> {
        let transport = MockTransport::new();
        let channel = transport.channel(ChannelPurpose::Control).await?;

        // Test sending data
        let test_data = b"Hello, Channel!";
        channel.send(test_data).await?;

        // MockChannel doesn't actually receive, but we can verify send doesn't error
        assert!(channel.is_open());

        Ok(())
    }

    #[test_timeout::tokio_timeout_test]
    async fn test_channel_reliability_settings() {
        // Test reliability enum
        let reliable = ChannelReliability::Reliable;
        let unreliable = ChannelReliability::Unreliable {
            max_retransmits: Some(0),
            max_packet_lifetime: None,
        };

        assert_eq!(reliable, ChannelReliability::Reliable);
        assert_ne!(reliable, unreliable);

        // Test default reliability for purposes
        assert_eq!(
            ChannelPurpose::Control.default_reliability(),
            ChannelReliability::Reliable
        );
        assert_eq!(
            ChannelPurpose::Output.default_reliability(),
            ChannelReliability::Unreliable {
                max_retransmits: Some(0),
                max_packet_lifetime: None,
            }
        );
    }

    #[test_timeout::tokio_timeout_test]
    async fn test_channel_options() {
        // Test channel options builder
        let control_opts = ChannelOptions::control();
        assert_eq!(control_opts.purpose, ChannelPurpose::Control);
        assert_eq!(control_opts.reliability, None);
        assert_eq!(control_opts.label, None);

        let output_opts = ChannelOptions::output()
            .with_reliability(ChannelReliability::Reliable)
            .with_label("custom-label".to_string());

        assert_eq!(output_opts.purpose, ChannelPurpose::Output);
        assert_eq!(output_opts.reliability, Some(ChannelReliability::Reliable));
        assert_eq!(output_opts.label, Some("custom-label".to_string()));
    }

    #[test_timeout::tokio_timeout_test]
    async fn test_channel_labels() {
        assert_eq!(ChannelPurpose::Control.label(), "beach/ctrl/1");
        assert_eq!(ChannelPurpose::Output.label(), "beach/term/1");
        assert_eq!(ChannelPurpose::Custom(42).label(), "beach/custom/42");
    }

    #[test_timeout::tokio_timeout_test]
    async fn test_backward_compatibility() -> Result<()> {
        let transport = MockTransport::new();

        // Old-style send should still work (uses control channel by default)
        let test_data = b"Legacy data";
        transport.send(test_data).await?;

        // Transport mode should still work
        assert_eq!(transport.transport_mode(), TransportMode::Server);
        assert!(transport.is_connected());

        Ok(())
    }
}
