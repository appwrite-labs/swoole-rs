"""GitHub workflow-command annotations: one line on stderr that names a failure on the run's
summary page, where `ci.py status` reads it back (check-runs/<job>/annotations). Printed
only under GitHub Actions, so local output stays as it is. No imports beyond the standard
library: gate.py runs on images without git."""
import os
import sys

# GitHub shows about 10 annotations of a level per step; past that, one more says how many.
PER_STEP = 10


def escape(text, separators=False):
    text = text.replace('%', '%25').replace('\r', '%0D').replace('\n', '%0A')
    return text.replace(':', '%3A').replace(',', '%2C') if separators else text


def annotate(level, title, message):
    """`::level title=title::message` ('error', 'warning' or 'notice')."""
    if os.environ.get('GITHUB_ACTIONS'):
        print(f'::{level} title={escape(title, True)}::{escape(message)}', file=sys.stderr, flush=True)


def annotate_each(level, title, messages, summary='the step summary lists them all'):
    """One annotation per message, the first PER_STEP - 1 by name and a count for the rest."""
    for message in messages[:PER_STEP - 1]:
        annotate(level, title, message)
    if len(messages) >= PER_STEP:
        annotate(level, title + 's', f'{len(messages) - PER_STEP + 1} more of {len(messages)}; {summary}')
