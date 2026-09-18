import shutil
import tempfile
from pathlib import Path

import pytest


@pytest.fixture
def dossier_court():
    """Dossier court sous /tmp : sous macOS, un chemin de socket Unix est limité à 104 octets."""
    d = Path(tempfile.mkdtemp(prefix="drc-", dir="/tmp"))
    yield d
    shutil.rmtree(d, ignore_errors=True)
