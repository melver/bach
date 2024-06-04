=============================
Bach: MIDI Command-Line Tools
=============================

Simple CLI tools to work with MIDI.

Usage
=====

TODO

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
