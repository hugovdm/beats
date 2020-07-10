fn main() {
    use cpal::traits::{DeviceTrait, HostTrait};
    // use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let _input_device = host
        .default_input_device()
        .expect("No input device available.");
    let output_device = host
        .default_output_device()
        .expect("No output device available.");

    let mut supported_configs_range = output_device
        .supported_output_configs()
        .expect("error while querying configs");
    let supported_config = supported_configs_range
        .next()
        .expect("no supported config?!")
        .with_max_sample_rate();

    // use cpal::{Data, Sample, SampleFormat};
    use cpal::{Sample, SampleFormat};
    use cpal::StreamConfig;
    let err_fn = |err| eprintln!("an error occurred on the output audio stream: {}", err);
    let sample_format = supported_config.sample_format();
    let config: StreamConfig = supported_config.into();

    println!("{:?}", config.sample_rate);

    let _stream = match sample_format {
        SampleFormat::F32 => output_device.build_output_stream(&config, write_silence::<f32>, err_fn),
        SampleFormat::I16 => output_device.build_output_stream(&config, write_silence::<i16>, err_fn),
        SampleFormat::U16 => output_device.build_output_stream(&config, write_silence::<u16>, err_fn),
    }
    .unwrap();

    fn write_silence<T: Sample>(data: &mut [T], _: &cpal::OutputCallbackInfo) {
        for sample in data.iter_mut() {
            *sample = Sample::from(&0.0);
        }
    }
}
