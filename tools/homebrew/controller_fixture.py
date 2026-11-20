"""Drive Coin Run through a temporary Linux Xbox-style gamepad and Proton."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack, contextmanager
from ctypes import byref
import os
import queue
import select
import threading
from pathlib import Path
import subprocess
import sys
import time

try:
    from evdev import AbsInfo, UInput, ecodes as e, ff
except ModuleNotFoundError as exc:
    raise SystemExit("The controller fixture needs the Python evdev package.") from exc


class RumblePad(UInput):
    def _find_device(self, fd):
        # Only Proton needs the event node. Opening it here can block behind a
        # force-feedback request before our uinput reader gets a chance to start.
        return None


def gamepad(rumble=False):
    keys = [e.BTN_SOUTH, e.BTN_EAST, e.BTN_NORTH, e.BTN_WEST, e.BTN_TL,
            e.BTN_TR, e.BTN_SELECT, e.BTN_START, e.BTN_MODE, e.BTN_THUMBL, e.BTN_THUMBR]
    axes = [(code, AbsInfo(0, low, high, 0, 0, 0))
            for codes, low, high in [((e.ABS_X, e.ABS_Y, e.ABS_RX, e.ABS_RY), -32768, 32767),
                                     ((e.ABS_Z, e.ABS_RZ), 0, 255),
                                     ((e.ABS_HAT0X, e.ABS_HAT0Y), -1, 1)]
            for code in codes]
    capabilities = {e.EV_KEY: keys, e.EV_ABS: axes}
    if rumble:
        capabilities[e.EV_FF] = [e.FF_RUMBLE]
    pad_type = RumblePad if rumble else UInput
    return pad_type(capabilities, name="TomBox virtual gamepad",
                    bustype=e.BUS_USB, vendor=0x045e, product=0x028e, version=0x0114)


def read_rumble(pad, changes, done):
    effects = {}
    playing = set()
    previous = (0, 0)
    while not done.is_set():
        if not select.select([pad.fd], [], [], 0.025)[0]:
            continue
        for event in pad.read():
            if event.type == e.EV_UINPUT and event.code == e.UI_FF_UPLOAD:
                # Some evdev releases put this ID in effect_id. Linux expects
                # request_id, which can be nonzero when two requests overlap.
                upload = ff.UInputUpload(request_id=event.value)
                if pad.dll._uinput_begin_upload(pad.fd, byref(upload)):
                    raise OSError(f"could not begin uinput upload request {event.value}")
                motors = upload.effect.u.ff_rumble_effect
                effects[upload.effect.id] = (motors.strong_magnitude, motors.weak_magnitude)
                upload.retval = 0
                pad.end_upload(upload)
            elif event.type == e.EV_UINPUT and event.code == e.UI_FF_ERASE:
                erase = ff.UInputErase(request_id=event.value)
                if pad.dll._uinput_begin_erase(pad.fd, byref(erase)):
                    raise OSError(f"could not begin uinput erase request {event.value}")
                playing.discard(erase.effect_id)
                effects.pop(erase.effect_id, None)
                erase.retval = 0
                pad.end_erase(erase)
            elif event.type == e.EV_FF and event.code in effects:
                if event.value:
                    playing.add(event.code)
                else:
                    playing.discard(event.code)
            # Uploading a new value can also change an already-playing effect.
            state = tuple(max((effects[i][axis] for i in playing), default=0) for axis in (0, 1))
            if state != previous:
                changes.put(state)
                previous = state


@contextmanager
def rumble_pad():
    changes = queue.Queue()
    done = threading.Event()
    with gamepad(rumble=True) as pad, ThreadPoolExecutor(max_workers=1) as pool:
        reader = pool.submit(read_rumble, pad, changes, done)
        try:
            yield pad, changes
        finally:
            done.set()
            reader.result()


def expect_rumble(changes, expected):
    try:
        observed = changes.get(timeout=3)
    except queue.Empty:
        raise TimeoutError(f"no Linux force-feedback transition for {expected}") from None
    # Some XInput backends quantize nonzero motor strengths to eight bits.
    if any(abs(a - b) > (256 if b else 0) for a, b in zip(observed, expected)):
        raise ValueError(f"expected rumble {expected}, got {observed}")


@contextmanager
def run_check(output, executable):
    output.unlink(missing_ok=True)
    command = f"{executable} > {output} 2>&1"
    process = subprocess.Popen([os.environ["PROTON"], "run", "cmd.exe", "/d", "/c", command])
    cursor = 0

    def expect(expected, timeout=8):
        nonlocal cursor
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            lines = output.read_text(errors="replace").split("\n")[:-1] if output.exists() else []
            while cursor < len(lines):
                line = lines[cursor]
                cursor += 1
                print(line, flush=True)
                if line.startswith("pad "):
                    if line not in expected:
                        raise ValueError(f"expected {expected}, got {line}")
                    return line
            if process.poll() is not None:
                raise RuntimeError(f"Controller check exited ({process.returncode}) before {expected}")
            time.sleep(0.025)
        raise TimeoutError(f"waiting for {expected}")

    try:
        yield expect
        if process.wait(timeout=10):
            raise RuntimeError(f"{executable} failed")
    finally:
        if process.poll() is None:
            # The Windows probes and the game have their own input deadlines.
            try:
                process.wait(timeout=35)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait()


def check_ordinal(output):
    with run_check(output, r"gameinput-xinput\gameinput_probe.exe") as expect:
        with rumble_pad() as (pad, motors):
            expect(["pad baseline"], timeout=45)
            for key in (e.BTN_A, e.BTN_X, e.BTN_TR, e.BTN_THUMBL, e.BTN_SELECT):
                pad.write(e.EV_KEY, key, 1)
            for axis, value in ((e.ABS_Z, 255), (e.ABS_RZ, 255), (e.ABS_X, -32768),
                                (e.ABS_Y, -32768), (e.ABS_RX, 32767), (e.ABS_RY, 32767)):
                pad.write(e.EV_ABS, axis, value)
            pad.syn()
            expect(["pad trigger pulse"], timeout=12)
            expect_rumble(motors, (0, 16384))
            pad.write(e.EV_KEY, e.BTN_START, 1)
            pad.syn()
            expect(["pad clamped pulse"])
            expect_rumble(motors, (65535, 0))
            pad.write(e.EV_KEY, e.BTN_START, 0)
            pad.syn()
            expect(["pad sampled"])
            expect_rumble(motors, (0, 0))
        expect(["pad detached"])
        with rumble_pad() as (pad, motors):
            expect(["pad replacement pulse"])
            expect_rumble(motors, (0, 32768))
            pad.write(e.EV_KEY, e.BTN_START, 1)
            pad.syn()
            expect(["pad stale handle checked"])
            try:
                stray = motors.get(timeout=0.25)
            except queue.Empty:
                pass
            else:
                raise ValueError(f"detached handle changed replacement rumble to {stray}")
            pad.write(e.EV_KEY, e.BTN_START, 0)
            pad.syn()
            expect(["pad replaced"])
            expect_rumble(motors, (0, 0))
    print("Verified ordinal 1, immutable readings, triggers and sticks, motor clamping, trigger mixing, stop and stale-handle isolation.")


def check_callbacks(output):
    with run_check(output, r"gameinput-xinput\gameinput_callbacks.exe") as expect:
        expect(["pad watching"], timeout=45)
        with gamepad():
            expect(["pad enumerated"], timeout=15)
            with gamepad():
                expect(["pad pair verified"])
            expect(["pad second detached"])
        expect(["pad callback detached"])
        with gamepad():
            expect(["pad callbacks verified"])
    print("Verified callback enumeration, filters, stop, timed unregister and replacement.")


def check_reading_callbacks(expect, primary, secondary):
    def send(pad, kind, code, value):
        pad.write(kind, code, value)
        pad.syn()
        time.sleep(0.025)

    expect(["pad subscriptions ready"])
    send(primary, e.EV_KEY, e.BTN_A, 1)
    for value in (9000, 15500, 29000, 0):
        send(secondary, e.EV_ABS, e.ABS_X, value)
    send(primary, e.EV_KEY, e.BTN_A, 0)
    expect(["pad unsubscribed"])
    for value in (1, 0):
        send(primary, e.EV_KEY, e.BTN_A, value)
    expect(["pad stall ready"])
    send(primary, e.EV_KEY, e.BTN_A, 1)
    expect(["pad subscriber stalled"])
    for i in range(91):
        send(primary, e.EV_KEY, e.BTN_A, i % 2)
    send(primary, e.EV_KEY, e.BTN_B, 1)
    expect(["pad reading callbacks verified"])


def check_history(output):
    # This file releases the paused input loop after both virtual pads are neutral.
    resume = output.with_name("resume.bin")
    command = rf"gameinput-xinput\gameinput_history.exe {resume}"
    try:
        with run_check(output, command) as expect, gamepad() as primary:
            expect(["pad history first"], timeout=45)
            with gamepad() as secondary:
                expect(["pad history paused"])
                for pad, kind, code, value in (
                    (primary, e.EV_KEY, e.BTN_A, 1), (secondary, e.EV_ABS, e.ABS_X, 30000),
                    (primary, e.EV_KEY, e.BTN_A, 0), (secondary, e.EV_ABS, e.ABS_X, 0),
                ):
                    pad.write(kind, code, value)
                    pad.syn()
                    time.sleep(0.06)
                resume.touch()
                expect(["pad history replayed"])
                check_reading_callbacks(expect, primary, secondary)
            expect(["pad history verified"])
    finally:
        resume.unlink(missing_ok=True)
    print("Verified forward and backward history, per-device replay, expiry gaps, filtered reading callbacks, analog thresholds, unregistration and overrun delivery.")


def check_route(output, adapter):
    command = (r"gameinput-xinput\coin_window.exe --gameinput-test" if adapter
               else "coin_window.exe --controller-test")

    def move(code, value, action, position, coins, hold=0):
        kind = e.EV_KEY if code == e.BTN_START else e.EV_ABS
        pad.write(kind, code, value)
        pad.syn()
        expect([f"pad {action} {position} {coins}"])
        time.sleep(hold)
        pad.write(kind, code, 0)
        pad.syn()
        if action != "r":
            expect(["pad neutral"])

    with run_check(output, command) as expect, ExitStack() as peer:
        with rumble_pad() as (pad, primary_motor):
            backends = ["pad ready GameInput"] if adapter else ["pad ready XInput", "pad ready GameInput"]
            ready = expect(backends, timeout=45)
            backup, backup_motor = peer.enter_context(rumble_pad())
            move(e.ABS_HAT0X, 1, "d", 1, 2, hold=0.4)
            move(e.ABS_HAT0X, 1, "d", 2, 2)
            move(e.ABS_HAT0Y, 1, "s", 2, 2)
            move(e.ABS_HAT0X, 1, "d", 3, 1)
            expect_rumble(primary_motor, (40960, 16384))
            expect_rumble(primary_motor, (0, 0))
            if not backup_motor.empty():
                raise ValueError("pickup drove the idle controller")
        expect([ready.replace("ready", "inactive")])
        pad = backup
        expect([ready])
        pad.write(e.EV_ABS, e.ABS_X, -10000)
        pad.syn()
        time.sleep(0.15)
        for position in (2, 1, 0):
            move(e.ABS_X, -28000, "a", position, 1)
        move(e.ABS_Y, 28000, "s", 4, 1)
        move(e.ABS_Y, 28000, "s", 8, 0)
        expect_rumble(backup_motor, (40960, 16384))
        expect_rumble(backup_motor, (0, 0))
        move(e.ABS_HAT0X, 1, "d", 8, 0)
        move(e.BTN_START, 1, "r", 0, 2)
        if not primary_motor.empty() or not backup_motor.empty():
            raise ValueError("unexpected extra rumble on the route")
        print("Verified controller input, both pickup pulses and stops at the Linux devices, and restart.")


if __name__ == "__main__":
    output = Path(sys.argv[1])
    adapter = sys.argv[2:] == ["--gameinput-test"]
    if adapter:
        check_ordinal(output)
        check_callbacks(output)
        check_history(output)
    check_route(output, adapter)
