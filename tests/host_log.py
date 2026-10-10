"""Strict UTF-8 snapshots of a log whose writer may still be running."""
import codecs


def read_running_log(path):
    # Each snapshot starts at byte zero. Only an incomplete trailing code point
    # may be pending; malformed complete bytes still raise UnicodeDecodeError.
    return codecs.getincrementaldecoder('utf-8')().decode(path.read_bytes(), final=False)
