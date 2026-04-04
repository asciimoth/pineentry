# PineEntry
GPG Pinentry caching proxy.  

PineEntry works as [assuan](https://velvetcache.org/2023/03/26/a-peek-inside-pinentry/) middleware between GPG and actual [pinentry program](https://www.gnupg.org/related_software/pinentry/index.html) that can:
- cache pins
- serve pins from static files
- serve pins from env vars

Check [config example](./example_config.yaml) for more features.

## Setup
Note that you should use absolute paths specific to your system.

Setup GPG agent:
```~/.gnupg/gpg-agent.conf
pinentry-program /path/to/pineentry
```

Create config in `~/.config/pineentry/config.yaml` usung [example one](./example_config.yaml) as reference.

## License
Files in this repository are distributed under the CC0 license.  

<p xmlns:dct="http://purl.org/dc/terms/">
  <a rel="license"
     href="http://creativecommons.org/publicdomain/zero/1.0/">
    <img src="http://i.creativecommons.org/p/zero/1.0/88x31.png" style="border-style: none;" alt="CC0" />
  </a>
  <br />
  To the extent possible under law,
  <a rel="dct:publisher"
     href="https://github.com/asciimoth">
    <span property="dct:title">ASCIIMoth</span></a>
  has waived all copyright and related or neighboring rights to
  <span property="dct:title">pineentry</span>.
</p>

