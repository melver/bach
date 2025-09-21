===============================
Bach: Evolutionary MIDI Library
===============================

*Bach* is a Rust library for algorithmic MIDI generation. The library contains
an `evolutionary algorithm`_ for automatic and interactive generation of MIDI
clips, allowing users to guide the creative process while exploring emergent
musical patterns.

Tools
=====

A few example CLI tools that make use of the library are included. The most
useful one is called `bach-evolve`_.

The tools included have only been tested on Linux-based systems. The core
``bach`` crate does not have any platform-specific dependencies.

bach-evolve
-----------

An interactive command-line debugger-style tool for evolving MIDI clips. The
basic syntax is::

   bach-evolve <config> <bpm> <ppqn> <midi-device>

Parameters:

* `<config>`: Configuration file path (use "-" for defaults)
* `<bpm>`: Beats per minute
* `<ppqn>`: Pulses per quarter note (typically 48)
* `<midi-device>`: MIDI output device

Interactive Commands
""""""""""""""""""""

Main Mode::

  a <count>              : auto-evolve next <count> generations
  bpm <val>              : change BPM
  c                      : continue
  chan <from> <to>...    : map channel <from> to one of <to>...
  e <idx>                : edit clip
  i                      : info
  l <count> <prefix>     : load <count> genomes into population
  mut <val>              : change mutation probability to <val>
  pfx <file>             : load prefix clip from <file>
  q                      : quit
  s/S <idx or @file>,... : play/loop chained clips (song mode)
  w <prefix>             : write population

Clip Editor Mode::

  a                : auto-score fitness value
  b                : back
  c <comment>      : comment
  d                : dump
  e <idx> = <inst> : edit instruction at index
  f/F [+=] <val>   : assign fitness value / ..back
  i                : info
  l <file>         : load from file
  p/P              : play / loop
  q                : quit
  w <file>         : write

Example Usage
"""""""""""""

Interactive Evolution::

   # start with default settings at 120 BPM
   $ bach-evolve - 120 48 /dev/snd/midiC0D2
   
   >>> a 10    # auto-evolve for 10 generations
   >>> i       # show population info
   
   >>> e best  # pick best-scoring clip
   >>> p       # play
   >>> b       # back to main
   
   >>> e 2     # edit clip #2
   >>> d       # dumps clip contents
   >>> p       # play

   >>> q       # quit

The evolutionary process maintains a population of MIDI clips, each with:

* fixed length (default 30 instructions);
* fitness score measuring musical quality;
* generation number tracking its evolution.

Connecting a Synthesizer
""""""""""""""""""""""""

The CLI tool writes the MIDI stream to a device file, which can be a hardware
synthesizer or a software synthesizer connected to a virtual device. For
example, we can use `fluidsynth <https://www.fluidsynth.org>`_ for quick
testing:

.. code-block:: sh

   # get a device we can use to pipe MIDI into
   $ modprobe snd-virmidi
   # start fluidsynth
   fluidsynth -a alsa -m alsa_seq -l FluidR3_GM.sf2
   # connect the ports
   aconnect 'Virtual Raw MIDI 0-2' 'FLUID Synth'
   # Use the raw MIDI port 0-2 to play music
   ... pipe raw MIDI to /dev/snd/midiC0D2 ...
   # disconnect when done
   aconnect -x

Evolutionary Algorithm
======================

Bach implements a **Genetic Programming (GP)** approach to evolve musical
sequences. Unlike traditional genetic algorithms that operate on fixed-length
bit strings, Bach evolves programs written in a domain-specific language (DSL).

Clip Representation
-------------------

Each musical clip is represented as a program consisting of instructions:

* **QueueNote**: Add a single note with pitch, velocity, and duration
* **QueueSequence**: Add euclidean sequence patterns with note lists
* **Tick**: Advance time by specified duration
* **Jump**: Branch forwards or backwards in the instruction sequence
* **QueueControl**: MIDI control change messages

This representation acts as a "compression" scheme that allows complex musical
patterns to emerge from relatively short programs.

Fitness Evaluation
------------------

Fitness scoring is based on music theory heuristics. The algorithm evaluates
clips across multiple dimensions:

**Harmonic Analysis**:

* *Vertical harmony*: Compatibility of simultaneous notes (chords) using
  interval-based scoring
* *Horizontal harmony*: Quality of note progressions over time
* Configurable harmony table maps semitone intervals to preference scores

**Musical Structure**:

* Channel balance across MIDI channels
* Repetition detection and scoring
* Rest/silence distribution
* Sequence density normalization

**Weights and Configuration**: All scoring components use configurable weights,
allowing users to bias evolution toward specific musical characteristics (e.g.,
emphasizing melody vs. harmony, encouraging or discouraging repetition).

Evolutionary Process
--------------------

The algorithm uses **steady-state evolution** with:

* **Crossover**: Program segments are exchanged between parent clips
* **Mutation**: Random instruction modifications at configurable probability
* **Tournament selection**: Best individuals from random subsets become parents
* **Replacement**: Delete the oldest replacement strategy

The process supports both manual fitness assignment (interactive evolution) and
automatic scoring based on the heuristics above.

License
=======

The Bach library is licensed under the terms of the Apache license. See LICENSE
for more information.
