"""Verify Coin Run's pickup tones on a temporary PulseAudio monitor."""
from array import array
from contextlib import ExitStack
from math import cos, pi, sin
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


RATE = 48000
FREQUENCIES = (440, 880)  # Dominant on the left and right, respectively.


def sound_intervals(samples):
    # Find non-silent runs in 5 ms blocks. Indices are stereo frames.
    block = RATE // 200
    first = None
    for frame in range(0, len(samples) // 2, block):
        chunk = samples[2 * frame:2 * (frame + block)]
        if sum(x * x for x in chunk) > len(chunk) * 32 ** 2:
            if first is None:
                first = frame
        elif first is not None:
            yield first, frame
            first = None
    if first is not None:
        raise ValueError("recording stopped in the middle of a sound")


def verify(raw):
    if len(raw) % 4:
        raise ValueError("incomplete stereo PCM frame")
    samples = array("h")
    samples.frombytes(raw)
    if sys.byteorder != "little":
        samples.byteswap()

    # The game collects right, left, restarts, collects left, right, then
    # submits both voices together as a separate mixer check.
    expected = [(1,), (0,), (0,), (1,), (0, 1)]
    intervals = list(sound_intervals(samples))
    if len(intervals) != len(expected):
        raise ValueError(f"expected four pickups and one mixed sound, found {len(intervals)} sounds")

    # Each 50 ms window contains exactly 22 and 44 full cycles. The projections
    # separate the two frequencies even when they play on the same channel.
    window = RATE // 20
    basis = []
    for hz in FREQUENCIES:
        angles = [2 * pi * hz * i / RATE for i in range(window)]
        basis.append([complex(cos(angle), sin(angle)) for angle in angles])
    solo_level = [0.0, 0.0]
    for number, ((start, end), sides) in enumerate(zip(intervals, expected), 1):
        duration = (end - start) * 1000 / RATE
        if not 100 <= duration <= 145:
            raise ValueError(f"sound {number} lasted {duration:g} ms (expected 120 ms)")
        center = (start + end) // 2
        for offset in (center - window, center):
            amplitudes = []
            for channel in (0, 1):
                data = samples[2 * offset + channel:2 * (offset + window):2]
                amps = [2 * sum(x * weight for x, weight in zip(data, basis[side])) / window
                        for side in sides]
                energy = sum(x * x for x in data)
                explained = window / 2 * sum(abs(amp) ** 2 for amp in amps)
                if explained < 0.90 * energy:
                    raise ValueError(f"unexpected frequencies in sound {number}, channel {channel}")
                amplitudes.append(amps)
            for index, side in enumerate(sides):
                direct = amplitudes[side][index]
                far = amplitudes[1 - side][index]
                # Compare phase as well as gain so an inverted channel fails.
                if abs(direct) < 32 or abs(far - 0.25 * direct) > 0.04 * abs(direct):
                    raise ValueError(f"incorrect stereo gain for {FREQUENCIES[side]} Hz in sound {number}")
                if len(sides) == 1:
                    solo_level[side] = abs(direct)
                elif not 0.8 <= abs(direct) / solo_level[side] <= 1.2:
                    raise ValueError(f"mixed level for {FREQUENCIES[side]} Hz differs from its solo pickup")
    print("Verified four stereo pickups (right, left, left, right), 4:1 panning, "
          "and both voices sounding together at their solo levels.")


def stop(process):
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def check(result):
    # The policy-only WirePlumber profile connects streams without discovering
    # hardware. All three services and their sockets belong to this test.
    with tempfile.TemporaryDirectory(prefix="tuxbox-audio-") as runtime, ExitStack() as cleanup:
        server = f"unix:{runtime}/pulse/native"
        private = dict(os.environ, XDG_RUNTIME_DIR=runtime, XDG_CONFIG_HOME=runtime,
                       XDG_STATE_HOME=runtime, PIPEWIRE_RUNTIME_DIR=runtime,
                       PIPEWIRE_REMOTE="pipewire-0", PULSE_SERVER=server)
        for command in (["pipewire"], ["wireplumber", "-p", "policy"], ["pipewire-pulse"]):
            process = subprocess.Popen(command, env=private)
            cleanup.callback(stop, process)
            # Wait for the core socket before launching its clients.
            if command == ["pipewire"]:
                wait_for_socket(Path(runtime, "pipewire-0"), process)
        wait_for_socket(Path(runtime, "pulse/native"), process)
        sink = "tombox_coin"
        subprocess.run(["pactl", "load-module", "module-null-sink", f"sink_name={sink}",
                        "rate=48000", "channels=2"], env=private, check=True,
                       stdout=subprocess.DEVNULL, timeout=10)

        with tempfile.TemporaryFile() as raw:
            recorder = subprocess.Popen([
                "parec", "--raw", f"--device={sink}.monitor", "--format=s16le",
                "--rate=48000", "--channels=2", "--channel-map=front-left,front-right", "--latency-msec=20",
            ], env=private, stdout=raw)
            try:
                env = dict(os.environ, PULSE_SERVER=server, PULSE_SINK=sink)
                run = subprocess.run([
                    env["PROTON"], "run", "cmd.exe", "/d", "/c",
                    f"coin_window.exe --audio-test > {result} 2>&1",
                ], env=env, timeout=90)
                if result.exists():
                    print(result.read_text(errors="replace"), end="", flush=True)
                run.check_returncode()
                time.sleep(0.2)  # Allow delivery of the last monitor blocks.
                if recorder.poll() is not None:
                    raise RuntimeError(f"monitor capture exited early ({recorder.returncode})")
            finally:
                stop(recorder)
            raw.seek(0)
            verify(raw.read())


def wait_for_socket(path, process):
    deadline = time.monotonic() + 5
    while not path.is_socket():
        if process.poll() is not None or time.monotonic() >= deadline:
            raise RuntimeError(f"{process.args[0]} failed to open {path}")
        time.sleep(0.05)


if __name__ == "__main__":
    check(Path(sys.argv[1]))
