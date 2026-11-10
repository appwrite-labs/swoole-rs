"""GitHub workflow-command annotations: one line on stderr that names a failure on the run's
summary page, where `ci.py status` reads it back (check-runs/<job>/annotations). Printed
only under GitHub Actions, so local output stays as it is. No imports beyond the standard
library: gate.py runs on images without git."""
import atexit
from collections import Counter
import os
import sys

# GitHub keeps about 10 annotations of a level per step. A process is one step's tool, so
# it names the first PER_STEP - 1 of each level and, at exit, counts the rest in one more.
PER_STEP = 10
_shown, _omitted = Counter(), Counter()


def escape(text, separators=False):
    text = text.replace('%', '%25').replace('\r', '%0D').replace('\n', '%0A')
    return text.replace(':', '%3A').replace(',', '%2C') if separators else text


def emit(level, title, message):
    print(f'::{level} title={escape(title, True)}::{escape(message)}', file=sys.stderr, flush=True)


def annotate(level, title, message):
    """`::level title=title::message` ('error', 'warning' or 'notice'), within the step's budget."""
    if not os.environ.get('GITHUB_ACTIONS'):
        return
    if _shown[level] < PER_STEP - 1:
        _shown[level] += 1
        emit(level, title, message)
        return
    if not _omitted:
        atexit.register(summarise)
    _omitted[level] += 1


def summarise():
    """The count of each level's annotations past the budget, which the log still lists."""
    for level, count in _omitted.items():
        emit(level, f'{count} more {level}s', f'{count} more past the first {PER_STEP - 1}; the log lists them all')
