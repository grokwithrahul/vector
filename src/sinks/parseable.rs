use vector_lib::codecs::encoding::FramingConfig;
use vector_lib::codecs::encoding::JsonSerializerConfig;
use vector_lib::codecs::encoding::JsonSerializerOptions;
use vector_lib::codecs::encoding::SerializerConfig;
use vector_lib::codecs::MetricTagValues;
use vector_lib::configurable::configurable_component;

use crate::{
    codecs::{EncodingConfigWithFraming, Transformer},
    config::{AcknowledgementsConfig, DataType, GenerateConfig, Input, SinkConfig, SinkContext},
    http::Auth,
    sinks::{
        http::config::{HttpMethod, HttpSinkConfig},
        util::{
            http::RequestConfig, BatchConfig, Compression, RealtimeSizeBasedDefaultBatchSettings, UriSerde,
        },
        Healthcheck, VectorSink,
    },
    tls::TlsConfig,
};


/// Configuration for the `parseable` sink.
#[configurable_component(sink("parseable", "Deliver log events to Parseable."))]
#[derive(Clone, Debug, Default)]
pub struct ParseableConfig {
    /// The full URI to make HTTP requests to.
    ///
    /// This should include the protocol and host, but can also include the port, path, and any other valid part of a URI.
    #[configurable(metadata(docs::examples = "https://10.22.212.22:9000/endpoint"))]
    uri: UriSerde,

    #[configurable(derived)]
    pub auth: Option<Auth>,

    ///The Parseable stream to send log events to.
    #[configurable(derived)] 
    stream: String,
    
    #[configurable(derived)]
    #[serde(default)]
    method: HttpMethod,

    #[configurable(derived)]
    #[serde(default)]
    request: RequestConfig,

    /// The compression algorithm to use.
    #[configurable(derived)]
    #[serde(default)]
    compression: Compression,

    /// The TLS settings for the connection.
    ///
    /// Optional, constrains TLS settings for this sink.
    #[configurable(derived)]
    tls: Option<TlsConfig>,

    /// The batch settings for the sink.
    #[configurable(derived)]
    #[serde(default)]
    pub batch: BatchConfig<RealtimeSizeBasedDefaultBatchSettings>,

    /// Controls how acknowledgements are handled for this sink.
    #[configurable(derived)]
    #[serde(
        default,
        deserialize_with = "crate::serde::bool_or_struct",
        skip_serializing_if = "crate::serde::is_default"
    )]
    acknowledgements: AcknowledgementsConfig,
}

impl GenerateConfig for ParseableConfig {
    fn generate_config() -> toml::Value {
        toml::from_str(
            r#"uri = "https://10.22.212.22:9000/endpoint"
            stream = "test_stream"
            "#,
        )
        .unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "parseable")]
impl SinkConfig for ParseableConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        let mut request = self.request.clone();
        request
            .headers
            .insert("X-p-stream".to_string(), self.stream.clone());

        let http_sink_config = HttpSinkConfig {
            uri: self.uri.clone(),
            compression: self.compression,
            auth: self.auth.clone(),
            method: self.method.clone(),
            tls: self.tls.clone(),
            request,
            acknowledgements: self.acknowledgements,
            batch: self.batch,
            headers: None,
            encoding: EncodingConfigWithFraming::new(
                Some(FramingConfig::NewlineDelimited),
                SerializerConfig::Json(JsonSerializerConfig {
                    metric_tag_values: MetricTagValues::Single,
                    options: JsonSerializerOptions { pretty: false }, // Minified JSON
                }),
                Transformer::default(),
            ),
            payload_prefix: "".into(), // Always newline delimited JSON
            payload_suffix: "".into(), // Always newline delimited JSON
        };

        http_sink_config.build(cx).await
    }

    fn input(&self) -> Input {
        Input::new(DataType::Metric | DataType::Log)
    }

    fn acknowledgements(&self) -> &AcknowledgementsConfig {
        &self.acknowledgements
    }
}

#[cfg(test)]
mod test { 
    use hyper::{Method};
    use super::*;
    use vector_lib::event::{BatchNotifier, BatchStatus};
    use crate::{
        config::SinkContext,
        test_util::components::{run_and_assert_sink_compliance, HTTP_SINK_TAGS},
        test_util::{random_lines_with_stream, next_addr},
        sinks::{parseable::ParseableConfig, 
            util::{
                test::{build_test_server, get_received_gzip},
            }
        }
    };
    async fn run_sink(extra_config: &str, assert_parts: impl Fn(http::request::Parts)) {
        let num_lines = 1000;

        let (in_addr, sink) = build_sink(extra_config).await;

        let (rx, trigger, server) = build_test_server(in_addr);
        tokio::spawn(server);

        let (batch, mut receiver) = BatchNotifier::new_with_receiver();
        let (input_lines, events) = random_lines_with_stream(100, num_lines, Some(batch));
        run_and_assert_sink_compliance(sink, events, &HTTP_SINK_TAGS).await;
        drop(trigger);

        assert_eq!(receiver.try_recv(), Ok(BatchStatus::Delivered));

        let output_lines = get_received_gzip(rx, assert_parts).await;

        assert_eq!(num_lines, output_lines.len());
        assert_eq!(input_lines, output_lines);
    }

    async fn build_sink(extra_config: &str) -> (std::net::SocketAddr, crate::sinks::VectorSink) {
        let in_addr = next_addr();
        let config = format!( 
            r#"
                uri = "http://{addr}/frames"
                compression = "gzip"
                framing.method = "newline_delimited"
                encoding.codec = "json"
                stream = "test-stream"
                {extras}
            "#,
            addr = in_addr,
            extras = extra_config,
        );
        let config: ParseableConfig = toml::from_str(&config).unwrap();

        let cx = SinkContext::default();

        let (sink, _) = config.build(cx).await.unwrap();
        (in_addr, sink)
    }
    #[tokio::test]

    async fn parseable_check_stream_header() { 
        run_sink(
            r#"
            method = "put"
        "#,
            |parts| {
                assert_eq!(Method::PUT, parts.method);
                assert_eq!("/frames", parts.uri.path());
                assert_eq!(
                    Some("test-stream"),
                    parts.headers.get("X-p-stream").map(|v| v.to_str().unwrap())
                );
            },
        )
        .await;
    }
}    

