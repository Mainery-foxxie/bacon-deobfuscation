import glob
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))


def sample_files():
    """protected scripts to test against: $BGDEOBF_SAMPLES or tests/samples/ (not shipped; put your own there)"""
    d = os.environ.get('BGDEOBF_SAMPLES') or os.path.join(os.path.dirname(os.path.abspath(__file__)), 'samples')
    return sorted(glob.glob(os.path.join(d, '*.lua')) + glob.glob(os.path.join(d, '*.txt')))
