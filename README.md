# beats
Experimental repository for some audio code written in Rust.

Primary purpose: to help me learn Rust, while also playing with audio and signal
processing code and ideas that tickle my fancy.

## Current status
I'm essentially busy figuring out the tech stack. The app runs a feedback loop
like in cpal's examples/feedback.rs. By default, it plays back what it picked up
via the microphone, with a 1 second delay (flag-adjustable).

It also runs a Rocket-based webserver which serves various pages, including some
Dart code able to render an envelope of the last few seconds of audio.

## Roadmap
Things I'm likely to do next:

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
* Playing back from the recording, rather than just looping via a ring buffer.
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

## Building Dart UI
These instructions are probably not complete (an explicit `pub get` might be
needed? - TBD):
```sh
cd dart
pub global activate webdev
webdev build
cd ..
```

## Running
Since the project is not on Rocket v0.5+ yet, it currently still needs nightly
Rust rather than stable:
```
$ rustup override set nightly
```

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

## Design Ideas
### Sample Rates

Consider the prime factors of some numbers you've seen a lot:

* 48 = 2×2×2×2×3: at 48 kHz, one can divide a millisecond's worth of samples
  into 16ths, or into 3.
* 60 = 2×2×3×5: nicely divisible by any integer up to 6, but can be halved only
  twice. (120? 240? Good if we're not looking at triplets-of-triplets.)
* 360 = 2×2×2×3×3×5: in case you ever wondered why 360 is a nice number. Not
  divisible by 7 or 11, but divisible by all other integers up to 12.

When sample-perfect subdivisions are useful, there can be a good reason to make
a loop a multiple of one of these numbers.
