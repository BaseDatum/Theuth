-- 210 FOLLOW-A (M1): a dir share's root as the owner approved it (canonical path, device, inode),
-- local to the hosting engine and never replicated. A mount rebuilt after a restart is built from
-- this pin and the share stops when the path names another directory.
CREATE TABLE share_pins (
    share TEXT PRIMARY KEY,
    canon TEXT NOT NULL,
    dev INTEGER NOT NULL,
    ino INTEGER NOT NULL
);
