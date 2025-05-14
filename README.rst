=============================
Bach: MIDI Command-Line Tools
=============================

Simple CLI tools to work with MIDI clips.

Usage
=====

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
--------------------

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
-------------

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

* fixed length (default 30 beats);
* fitness score measuring musical quality;
* generation number tracking its evolution.

Testing
=======

It helps having a software synthesizer. On Linux this can be done with the help
of `fluidsynth <https://www.fluidsynth.org>`_:

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

License
=======

The Bach library is licensed under the terms of the Apache license. See LICENSE
for more information.
