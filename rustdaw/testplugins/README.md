Two tiny VST3 plugins (built with the `vst3` crate) used to test the host:
`gain` (effect, from the vst3-rs example) and `testsynth` (sine instrument driven by MIDI events).

    cd testplugins/gain && cargo build --release
    # bundle: gain.vst3/Contents/<x86_64-win | x86_64-linux>/gain.<vst3|so>
    # then:  RUSTDAW_TEST_VST3=<folder with the .vst3 bundles> cargo test -p plugin-host
