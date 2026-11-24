# Directory listings, without the surprise memory spike

One last security follow-up for the SFTP panel. Listing a folder used to ask the
SFTP library for everything at once: it read the entire directory to the end,
and only then did RuTerm apply the 10,000-entry limit. A hostile or simply very
large remote folder could therefore push a lot of data into memory before the
limit ever had a say.

RuTerm now opens a second, lightweight SFTP channel dedicated to directory
reads. Entries arrive one packet at a time, the 10,000-entry cap is enforced as
they arrive, and the remote directory handle is closed immediately when the cap
is hit. Recursive deletes use the same stream, so their budget is charged per
entry instead of after a full listing has already been collected.

The panel also no longer asks the server to "canonicalize" the folder it is
about to show. It uses the path you requested and builds each entry's path
locally, with the same filename checks as before. A server still controls its
own filesystem, but it no longer gets to hand us paths we would blindly trust.

Nothing changes in how the file panel looks. Eight new tests cover packet
decoding, the 10,000-entry boundary, error and EOF handling, and timeouts.
