"""Kernel stop receipts for a test-owned direct child; a marker is not a stop."""
import os
import signal
import time


def wait_stopped(process, deadline):
    """Consume SIGSTOP observation within the caller's existing absolute budget."""
    assert process.returncode is None, ('child exited before stop', process.returncode)
    while time.monotonic() < deadline:
        pid, status = os.waitpid(process.pid, os.WUNTRACED | os.WNOHANG)
        if pid:
            if os.WIFSTOPPED(status):
                assert os.WSTOPSIG(status) == signal.SIGSTOP, ('unexpected stop signal', status)
                assert time.monotonic() <= deadline, 'child stop observation deadline exceeded'
                return
            process.returncode = os.waitstatus_to_exitcode(status)
            raise AssertionError(('child exited before stop', process.returncode))
        time.sleep(.01)
    raise AssertionError('child stop observation deadline exceeded')
