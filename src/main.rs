extern crate hound;
extern crate num;

use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::io::Seek;
use std::io::Write;
use std::process::exit;
use std::time::Instant;

use hound::SampleFormat;
use hound::WavWriter;
use num::clamp;

use std::path::Path;

use symphonia::core::audio::{Channels, RawSampleBuffer, SignalSpec};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use csv::ReaderBuilder;
use samplerate::{convert, ConverterType};

#[derive(Debug)]
struct AudioSampleInfo {
    time: f32,
    volume: f32,
    pan: f32,
    name: String,
}

#[derive(Debug)]
struct AudioSample {
    info: AudioSampleInfo,
    data: Vec<f32>,
}

struct Config {
    input: String,
    output: String,
    quality: f32,
}

fn parse_arguments() -> Option<Config> {
    let args: Vec<String> = env::args().collect();

    // Check if there are enough arguments
    if args.len() < 5 {
        println!("Usage: {} -i <input_csv_file> -o <output_ogg_file>", args[0]);
        println!("\tOptional: -q <output_ogg_quality>\t(Default: 0.7)");
        return None;
    }

    // Parse arguments
    let mut input_path = "";
    let mut output_path = "";
    let mut quality_str = "0.7";

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-i" => {
                i += 1;
                if i < args.len() {
                    input_path = &args[i];
                }
            }
            "-o" => {
                i += 1;
                if i < args.len() {
                    output_path = &args[i];
                }
            }
            "-q" => {
                i += 1;
                if i < args.len() {
                    quality_str = &args[i];
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Print input and output paths
    println!("Input Path: {}", input_path);
    println!("Output Path: {}", output_path);
    println!("Output Quality: {}", quality_str);

    Some(Config {
        input: input_path.to_owned(),
        output: output_path.to_owned(),
        quality: quality_str.parse::<f32>().expect("could not parse quality to f32."),
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = parse_arguments();

    if config.is_none() {
        exit(1)
    }

    let config = config.unwrap();

    let mut rdr = ReaderBuilder::new().has_headers(false).from_path(config.input)?;

    let mut infos = Vec::new();

    for result in rdr.records() {
        let record = result?;
        let time: f32 = record[0].parse()?;
        let volume: f32 = record[1].parse()?;
        let pan: f32 = record[2].parse()?;
        let name = record[3].to_string();

        let new_record = AudioSampleInfo {
            time,
            volume,
            pan,
            name,
        };
        infos.push(new_record);
    }

    let mut sample_map = HashMap::with_capacity(infos.len());
    let mut timing_map = HashMap::with_capacity(infos.len());

    for info in infos {
        add_timing(&info.name, info.time, info.volume, info.pan, &mut timing_map);

        if !sample_map.contains_key(&info.name) {
            println!("{}", &info.name);
            let data = read_audio(&info.name);
            let data = data.expect("welp");
            let sample = AudioSample { info, data };
            sample_map.insert(sample.info.name.clone(), sample);
        }
    }

    let max_length = calculate_max_length(&sample_map, &timing_map);

    let mut data = vec![0 as f32; max_length];

    for (name, list) in timing_map.iter() {
        let sample = sample_map.get(name);

        if let Some(sample) = sample {
            for (index, volume, pan) in list.iter() {
                // println!("mix at {}", index);
                mix(&mut data, &sample.data, *index, *volume, *pan);
            }
        }
    }

    for element in data.iter_mut() {
        *element = clamp(*element, -1.0, 1.0);
    }

    export(&data, &config.output, config.quality)?;

    Ok(())
}

fn export(data: &[f32], output_file: &str, quality: f32) -> Result<(), Box<dyn Error>> {
    println!("exporting to {}", &output_file);
    let pcm_data: Vec<i16> = data.iter().map(|&x| (x * i16::MAX as f32) as i16).collect();

    let mut encoder = vorbis_encoder::Encoder::new(2, 44100, quality).expect("could not create vorbis encoder");
    let buffer = encoder.encode(&pcm_data).expect("could not encode data");

    let mut ogg_file = File::create(output_file)?;
    ogg_file.write_all(&buffer)?;
    Ok(())
}

fn mix(data: &mut [f32], sample: &[f32], index: usize, volume: f32, pan: f32) {
    let start_pos = 0;
    (start_pos..sample.len()).for_each(|i| {
        let a = data[index + i];
        let b = sample[i];

        let mut panning = 1.0;

        if pan != 0.0 {
            if i % 2 == 0 {
                // left channel
                panning = (1.0 - pan).min(1.0).max(0.0);
            } else {
                // right channel
                panning = (1.0 + pan).min(1.0).max(0.0);
            }
        }

        let value = a + b * volume * panning;
        data[index + i] = value;
    });
}

fn add_timing(
    wav_name: &str,
    ms: f32,
    volume: f32,
    pan: f32,
    timing_map: &mut HashMap<String, Vec<(usize, f32, f32)>>,
) {
    let offset = to_byte_offset(ms) as usize;

    if let Some(list) = timing_map.get_mut(wav_name) {
        // if !list.iter().any(|tuple| tuple.0 == offset) {
        list.push((offset, volume, pan));
        // }
    } else {
        timing_map.insert(wav_name.to_string(), vec![(offset, volume, pan)]);
    }
}

fn calculate_max_length(
    wav_map: &HashMap<String, AudioSample>,
    timing_map: &HashMap<String, Vec<(usize, f32, f32)>>,
) -> usize {
    let mut max_length = 0_usize;

    for (wav_name, audio_sample) in wav_map {
        let list = timing_map.get(wav_name);
        match list {
            None => {}
            Some(list) => {
                let max = list.iter().map(|v| v.0).max().unwrap_or(0);
                max_length = max_length.max(max + audio_sample.data.len());
            }
        }
    }

    max_length
}

fn to_byte_offset(ms: f32) -> i32 {
    let val = (ms / 1000.0 * 44100.0 * 2.0) as i32;
    if val % 2 != 0 {
        val - 1
    } else {
        val
    }
}

fn read_audio(path: &str) -> Result<Vec<f32>, symphonia::core::errors::Error> {
    // Open the media source.
    let src = std::fs::File::open(path).expect("failed to open media");

    // Create the media source stream.
    let mss = MediaSourceStream::new(Box::new(src), Default::default());

    // Create a probe hint using the file's extension. [Optional]
    let mut hint = Hint::new();
    let ext = Path::new(path).extension().and_then(|ext| ext.to_str()).unwrap();
    hint.with_extension(ext);

    // Use the default options for metadata and format readers.
    let meta_opts: MetadataOptions = Default::default();
    let fmt_opts: FormatOptions = Default::default();

    // Probe the media source.
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &fmt_opts, &meta_opts)
        .expect("unsupported format");
    // Get the instantiated format reader.
    let mut format = probed.format;

    // Find the first audio track with a known (decodeable) codec.
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .expect("no supported audio tracks");

    // Use the default options for the decoder.
    let dec_opts: DecoderOptions = Default::default();

    // Create a decoder for the track.
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &dec_opts)
        .expect("unsupported codec");

    // Store the track identifier, it will be used to filter packets.
    let track_id = track.id;

    let mut data = Vec::new();

    let mut not_stereo = false;

    let mut sample_rate = 0;

    // The decode loop.
    loop {
        // Get the next packet from the media format.
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::ResetRequired) => {
                // The track list has been changed. Re-examine it and create a new set of decoders,
                // then restart the decode loop. This is an advanced feature and it is not
                // unreasonable to consider this "the end." As of v0.5.0, the only usage of this is
                // for chained OGG physical streams.
                unimplemented!();
            }
            Err(err) => {
                // A unrecoverable error occured, halt decoding.\
                // This is totally how you do it
                if err.to_string() == "end of stream" {
                    break;
                }
                panic!("{}", err);
            }
        };

        // Consume any new metadata that has been read since the last packet.
        while !format.metadata().is_latest() {
            // Pop the old head of the metadata queue.
            format.metadata().pop();

            // Consume the new metadata at the head of the metadata queue.
        }

        // If the packet does not belong to the selected track, skip over it.
        if packet.track_id() != track_id {
            continue;
        }

        // Decode the packet into audio samples.
        match decoder.decode(&packet) {
            Ok(decoded) => {
                sample_rate = decoded.spec().rate;
                // Consume the decoded audio samples (see below).
                let spec = SignalSpec {
                    channels: Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
                    rate: 44100,
                };
                // Create a raw sample buffer that matches the parameters of the decoded audio buffer.
                let mut byte_buf = RawSampleBuffer::<f32>::new(decoded.capacity() as u64, spec);

                let num_channels = decoded.spec().channels.count();

                if !not_stereo && num_channels != 2 {
                    not_stereo = true;
                }

                // Copy the contents of the decoded audio buffer into the sample buffer whilst performing
                // any required conversions.
                byte_buf.copy_interleaved_ref(decoded);

                // The interleaved f32 samples can be accessed as a slice of bytes as follows.
                let bytes = byte_buf.as_bytes();
                // println!("{:?}", bytes.len());

                for chunk in bytes.chunks(4) {
                    if chunk.len() == 4 {
                        let f32_value = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        data.push(f32_value);
                        if num_channels == 1 {
                            data.push(f32_value);
                        }
                    } else {
                        println!("Warning: Ignoring incomplete chunk {:?}", chunk);
                    }
                }
            }

            Err(symphonia::core::errors::Error::IoError(_)) => {
                // The packet failed to decode due to an IO error, skip the packet.
                continue;
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => {
                // The packet failed to decode due to invalid data, skip the packet.
                continue;
            }
            Err(err) => {
                // An unrecoverable error occured, halt decoding.
                panic!("{}", err);
            }
        }
    }

    if not_stereo {
        println!("Not stereo. Attempting to fix.");
    }

    if sample_rate != 44100 {
        println!("Resampling {} to 44100.", sample_rate);
        // let mut output = vec![0_f32; 0];
        // resample(&data, &mut output, sample_rate as i32, 44100);

        let result = convert(sample_rate, 44100, 2, ConverterType::SincBestQuality, &data);
        data = result.expect("error resampling");

        // data = output;
    }

    Ok(data)

    // to_wav(&mut data);
}

fn to_wav(samples: &[f32], output_file: &str) -> Result<(), hound::Error> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44100,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };

    let mut writer = WavWriter::create(output_file, spec)?;

    for sample in samples {
        // Write the sample to both channels (since it's dual-channel)
        writer.write_sample(*sample)?;
        // println!("writing {}", sample);
    }

    Ok(())
}
