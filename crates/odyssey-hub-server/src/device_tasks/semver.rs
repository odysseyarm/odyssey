pub(super) fn semver_at_least(version: [u16; 3], required: [u16; 3]) -> bool {
    version[0] > required[0]
        || (version[0] == required[0] && version[1] > required[1])
        || (version[0] == required[0] && version[1] == required[1] && version[2] >= required[2])
}
