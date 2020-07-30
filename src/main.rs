fn main() {
    use cpal::traits::{DeviceTrait, HostTrait};
    // use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let input_device = host
        .default_input_device()
        .expect("No input device available.");
    let output_device = host
        .default_output_device()
        .expect("No output device available.");

    let mut supported_input_configs_range = input_device
        .supported_input_configs()
        .expect("error while querying input configs");
    let supported_input_config = supported_input_configs_range
        .next()
        .expect("no supported input config?!")
        .with_max_sample_rate();

    let mut extra_config_opt = supported_input_configs_range.next();
    while extra_config_opt != None {
        let extra_config = extra_config_opt
            .expect("error while iterating over supported configs")
            .with_max_sample_rate();
        println!("{:?}", extra_config);
        extra_config_opt = supported_input_configs_range.next();
    }
    println!();

    let mut supported_configs_range = output_device
        .supported_output_configs()
        .expect("error while querying configs");
    let supported_config = supported_configs_range
        .next()
        .expect("no supported config?!")
        .with_max_sample_rate();

    let mut extra_config_opt = supported_configs_range.next();
    while extra_config_opt != None {
        let extra_config = extra_config_opt
            .expect("error while iterating over supported configs")
            .with_max_sample_rate();
        println!("{:?}", extra_config);
        extra_config_opt = supported_configs_range.next();
    }
    println!();

    println!("supported_input_config was: {:?}", supported_input_config);
    println!("supported_config was: {:?}", supported_config);

    // use cpal::{Data, Sample, SampleFormat};
    use cpal::{Sample, SampleFormat};
    let err_fn = |err| eprintln!("an error occurred on the output audio stream: {}", err);
    let sample_format = supported_config.sample_format();
    // TODO: check against supported_input_config.sample_format();

    let mut input_config: cpal::StreamConfig = supported_input_config.into();
    // Overriding values anyway:
    input_config.channels = 1;
    input_config.sample_rate = cpal::SampleRate(44100);
    println!("using input_config: {:?}", input_config);

    let mut config: cpal::StreamConfig = supported_config.into();
    // Overriding values anyway:
    config.channels = 1;
    config.sample_rate = cpal::SampleRate(44100);
    println!("using config: {:?}", config);

    let _stream = match sample_format {
        SampleFormat::F32 => {
            // input_device.build_input_stream(&config, read_samples::<f32>, err_fn)
            output_device.build_output_stream(&config, write_silence::<f32>, err_fn)
        }
        SampleFormat::I16 => {
            // input_device.build_input_stream(&config, read_samples::<i16>, err_fn)
            output_device.build_output_stream(&config, write_silence::<i16>, err_fn)
        }
        SampleFormat::U16 => {
            // input_device.build_input_stream(&config, read_samples::<u16>, err_fn)
            output_device.build_output_stream(&config, write_silence::<u16>, err_fn)
        }
    };
    if let Err(e) = _stream {
        println!("Error from build_output_stream: {}", e);
        return ();
    }
    let _stream = _stream.unwrap();

    fn write_silence<T: Sample>(data: &mut [T], _: &cpal::OutputCallbackInfo) {
        for sample in data.iter_mut() {
            *sample = Sample::from(&0.0);
        }
    }
}
