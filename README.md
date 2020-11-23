# beats
Experimental repository for some audio code written in Rust.

Primary purpose: to help me learn Rust, while also playing with audio and signal
processing code and ideas that tickle my fancy.

## Current status
I'm essentially busy figuring out the tech stack. The app runs a feedback loop
like in cpal's examples/feedback.rs. By default, it plays back what it picked up
via the microphone, with a 1 second delay.

It also starts up a window and prints "wgpu_glyph" to it: when this window is
closed, the loop terminates. Throwing this out is probably the next change.

## Roadmap
Things I'm likely to do next:

* Rocket-based web server to serve a Dart-based UI.
  * Figuring out the tech stack: ability to interact with it via various devices
    (e.g. mobile phones) while the software could be running on e.g. a headless
    and input-device-less Raspberry Pi.
* Frequency sweep to measure the round-trip gain at various frequencies, and set
  gain based on that. (Open-loop automatic gain determination, I don't want to
  delve into closed-loop control systems at this time. :-)
  * I'm finding round-trip characteristics quite interesting, and might be
    helpful in seeing the differences when using different microphones and
    speakers.
  * Might help me play around and characterise the accoustics of my apartment
    too. It's prone to standing waves: can play around and see which points
    support which resonant frequencies, and how various interventions might help
    for improving the accoustic properties of the rooms?
* Beat detection - e.g. measuring the tempo.
* Recording for many minutes rather than just looping via a ring buffer.
  * Will permit playing back more selectively.
* Loop detection: see when something is repeated.
  * I wish to have the feedback delay determined by the measurement of a loop
    like this.
* Maybe playing with tempo modification?
  * Primitive: resampling (affecting pitch).
  * Maybe later: fancier algorithms for pitchless tempo adjustment? Could
    consider the [algorithms used by
    Audacity](https://wiki.audacityteam.org/wiki/SoundTouch), depending on their
    trade-offs
* More. (Where I'd actually like to take this little project remains
  undocumented, since I'm likely to never get that far, or for it to not really
  work as dreamed.)

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

## More Tips and Observations

### CPAL
Obtain a list of supported input and output formats by running the `enumerate`
example in the CPAL git repository:
```
cargo run --example enumerate
```

### CPAL Observations
On my system, the maximum f32 value coming from the default driver's input
(using PulseAudio?) seems to be 0.302. Using JACK, I get a cleaner 1.0.

These values were found experimentally, by observing the system clipping.
(Easiest way: carefully crank up the gain until one gets at least one frequency
slowly growing in volume.)

To be determined: can this maximum value be found without empirically observing
the audio system clipping? And does this maximum value apply both to input and
output equally?
