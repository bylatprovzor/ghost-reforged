"""Bound generated Warcraft/Battle.net diagnostics; never touch maps or match data."""
import argparse
import ctypes
import json
import os
import re
import shutil
import stat
import time
from dataclasses import dataclass
from pathlib import Path

MIB = 1024 * 1024
DAY = 86400


@dataclass(frozen=True)
class Policy:
    name: str
    root: Path
    pattern: str
    age_days: int = 14
    limit_bytes: int = 256 * MIB
    minimum_age_days: int = 2
    keep_newest: int = 2


def plain_path(path):
    """Refuse symlinks/junctions anywhere along the existing path."""
    path = Path(os.path.abspath(path))
    for parent in [path, *path.parents]:
        try:
            info = parent.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(info.st_mode) or getattr(info, 'st_file_attributes', 0) & 0x400:
            return False
    return True


def policies(profile, program_data):
    local = profile / 'AppData/Local'
    game = profile / 'Documents/Warcraft III'
    rows = [
        Policy('warcraft_logs', game / 'Logs', r'.*\.(?:log|txt)'),
        Policy('warcraft_errors', game / 'Errors', r'.*\.(?:dmp|mdmp|log|txt)', 14, 512*MIB),
        Policy('windows_game_dumps', local / 'CrashDumps',
               r'(?:Warcraft III|Battle\.net|BlizzardBrowser|Agent)\.exe\.[0-9]+\.dmp', 14, 512*MIB),
        Policy('launcher_logs', local / 'Battle.net/Logs', r'.*\.log'),
        Policy('browser_logs', local / 'Battle.net/BlizzardBrowser/Logs', r'.*\.log'),
    ]
    agent = program_data / 'Battle.net/Agent'
    if plain_path(agent) and agent.is_dir():
        for version in agent.iterdir():
            if re.fullmatch(r'Agent\.[0-9]+', version.name) and plain_path(version):
                rows.append(Policy('agent_logs_' + version.name, version / 'Logs', r'.*\.log'))
    return rows


def candidates(policy):
    if not plain_path(policy.root) or not policy.root.is_dir():
        return []
    base = policy.root.resolve(strict=True)
    result = []
    for folder, directories, files in os.walk(base, followlinks=False):
        directories[:] = [name for name in directories if plain_path(Path(folder) / name)]
        for name in files:
            path = Path(folder) / name
            if not re.fullmatch(policy.pattern, name, re.IGNORECASE) or not plain_path(path):
                continue
            try:
                info = path.lstat()
                path.resolve(strict=True).relative_to(base)
            except (OSError, ValueError):
                continue
            if stat.S_ISREG(info.st_mode) and info.st_nlink == 1:
                result.append((path, info))
    return sorted(result, key=lambda item: item[1].st_mtime, reverse=True)


def delete_exclusive(path, expected):
    """Delete the inspected Windows file through its exclusively held handle."""
    if os.name != 'nt':
        raise RuntimeError('Applying maintenance requires Windows')
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.CreateFileW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32,
                                  ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
    kernel.CreateFileW.restype = ctypes.c_void_p
    kernel.SetFileInformationByHandle.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    # DELETE + FILE_READ_ATTRIBUTES; no sharing; OPEN_EXISTING; open reparse itself.
    handle = kernel.CreateFileW(str(path), 0x10080, 0, None, 3, 0x200000, None)
    if handle == ctypes.c_void_p(-1).value:
        return False
    try:
        actual = path.lstat()
        if not plain_path(path) or actual.st_nlink != 1 or (
            actual.st_ino, actual.st_size, actual.st_mtime_ns
        ) != (expected.st_ino, expected.st_size, expected.st_mtime_ns):
            return False
        remove = ctypes.c_bool(True)
        return bool(kernel.SetFileInformationByHandle(handle, 4, ctypes.byref(remove), ctypes.sizeof(remove)))
    finally:
        kernel.CloseHandle(handle)


def clean(policy, now, apply=False):
    rows = candidates(policy)
    remaining = sum(info.st_size for _, info in rows)
    result = dict(name=policy.name, files=len(rows), before_bytes=remaining,
                  eligible_bytes=0, removed_bytes=0, removed_files=0, skipped_files=0)
    # Preserve the newest diagnostics and every file modified in the last two days.
    for path, info in reversed(rows[policy.keep_newest:]):
        age = now - info.st_mtime
        if age < policy.minimum_age_days*DAY:
            continue
        if age < policy.age_days*DAY and remaining <= policy.limit_bytes:
            continue
        result['eligible_bytes'] += info.st_size
        if apply:
            try:
                deleted = delete_exclusive(path, info)
            except OSError:
                deleted = False
            if not deleted:
                result['skipped_files'] += 1
                continue
            result['removed_bytes'] += info.st_size
            result['removed_files'] += 1
        remaining -= info.st_size
    result['remaining_bytes'] = remaining
    return result


def run(profile, program_data, state, apply):
    if not all(plain_path(p) for p in (profile, program_data, state)):
        raise ValueError('Maintenance paths must not contain links or junctions')
    state.mkdir(parents=True, exist_ok=True)
    # Atomic creation excludes overlapping scheduled/manual maintenance runs.
    import msvcrt
    with (state / 'maintenance.lock').open('a+b') as lock:
        lock.seek(0)
        try:
            msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
        except OSError:
            return dict(status='already_running')
        try:
            now = time.time()
            before = shutil.disk_usage(profile).free
            outcomes = [clean(p, now, apply) for p in policies(profile, program_data)]
            after = shutil.disk_usage(profile).free
            receipt = dict(version=1, unix_seconds=now, applied=apply, policies=outcomes,
                           free_bytes_before=before, free_bytes_after=after,
                           low_disk=after < 3*1024**3,
                           removed_bytes=sum(r['removed_bytes'] for r in outcomes))
            target = state / ('maintenance-status.json' if apply else 'maintenance-preview.json')
            staged = target.with_suffix('.tmp')
            staged.write_text(json.dumps(receipt, indent=2), encoding='utf-8')
            os.replace(staged, target)
            return receipt
        finally:
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--state-dir', type=Path, required=True)
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    print(json.dumps(run(Path.home(), Path(os.environ['ProgramData']), args.state_dir, args.apply)))


if __name__ == '__main__':
    main()
