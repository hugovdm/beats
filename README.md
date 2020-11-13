# beats
Experimental repository for some audio code written in Rust.

## Running
With 200 milliseconds feedback latency:
```
cargo run --release -- --delay 200
```

## JACK Support
Enable JACK while building, and run with JACK:
```
cargo run --release --features jack -- --jack
```
This permits lower latency than PulseAudio.

## More Tips

### CPAL
Obtain a list of supported input and output formats by running the `enumerate`
example in the CPAL git repository:
```
cargo run --example enumerate
```
