# Audio-sample-mixer
Mixes audio samples with a given start time, volume and pan into a combined .ogg file.

Audio samples are resampled to 44100 Hz and mono audio is converted to stereo.

# Usage
```audio-sample-mixer.exe -i <input_csv_file> -o <output_ogg_file>``` 

Optional: ```-q <output_ogg_quality>``` (Default: 0.7)

# CSV Structure
```time,volume,pan,file```
- no header row
- **time** in miliseconds (float)
- **volume** factor from 0.0 to 1.0 (float)
- **pan** factor from -1.0 to 1.0, with 0.0 as center sound (float)
- **file** path to the respective sample file
<br>

Uses [Symphonia](https://github.com/pdeljanov/Symphonia) for audio decoding.
