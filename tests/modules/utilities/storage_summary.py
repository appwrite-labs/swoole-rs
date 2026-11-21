"""Compare the engines' Logger storage summaries strictly."""
import json


def compare(results):
    if len({json.dumps(row['summary'], sort_keys=True) for row in results}) != 1:
        raise ValueError('Native/selected Logger storage summaries differ')
