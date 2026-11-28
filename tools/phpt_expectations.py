"""Upstream's PHP 8.5 expectation selection (tests/run-tests), without its custom runner."""
import re

KINDS = ('EXPECT', 'EXPECTF', 'EXPECTREGEX')


def select(data, php_version_id):
    """The PHPT with its --EXPECT*_85-- body in the base section's place on PHP >= 8.5, else dropped."""
    if not isinstance(php_version_id, int) or php_version_id < 50000:
        raise ValueError('Require actual PHP_VERSION_ID')
    markers = list(re.finditer(rb'^--([A-Z][A-Z0-9_]*)--(?:\r?\n|$)', data, re.M))
    if not markers or markers[0].start() != 0:
        raise ValueError('Malformed PHPT section prefix')
    parts = {}
    for index, marker in enumerate(markers):
        name = marker.group(1).decode('ascii')
        if name in parts:
            raise ValueError('Duplicate PHPT section: ' + name)
        end = markers[index + 1].start() if index + 1 < len(markers) else len(data)
        parts[name] = (data[marker.start():marker.end()], data[marker.end():end])
    conditional = [kind + '_85' for kind in KINDS if kind + '_85' in parts]
    selected = {kind: kind + '_85' if php_version_id >= 80500 and kind + '_85' in parts else kind
                for kind in KINDS if kind in parts or php_version_id >= 80500 and kind + '_85' in parts}
    if len(selected) != 1:
        raise ValueError('Require one active original expectation kind')
    chunks = []
    for name, (marker, body) in parts.items():
        if name in conditional:
            if php_version_id >= 80500 and name.removesuffix('_85') not in parts:
                chunks.append(marker.replace(b'_85', b'') + body)
        elif name in selected:
            chunks.append(marker + parts[selected[name]][1])
        else:
            chunks.append(marker + body)
    return b''.join(chunks)
