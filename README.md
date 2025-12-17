ff
--

> [!WARNING]
> Work in progress. Use [npins](https://github.com/andir/npins) or [niv](https://github.com/nmattia/niv) instead.

## Why

Updating sources (i.e. the `src =` attribute in a derivation) is a messy job. If you do it manually, you'd need to update the commit hash, the version number, set nix hash to `lib.fakeHash`. Then run `nix build` and wait for it to give you an error, which you then copy and replace the `lib.fakeHash` with. Of course, people then invented `updateScript`, which attempts to automate this process. But, it's still messy, since `nix-update-script` has to evaluate nix expressions. Updating the nix file with the results is tricky too, since nix is a Turing complete language, there is no restriction on how `src` can be specified.

Some packages attempt to alleviate this by having a separate `source.json`/`sources.json` on the side, so the sources are specified somewhere outside the nix file and are therefore easier to update. But those are ad-hoc, and there's no standard for the structure of these files and what information should be contained in them.

I believe all packages in nixpkgs should adopt this practice, and there really should be a standard tool for doing this. So this is it.

Tools like `npins` or `niv` can be used to do similar things, but they aren't designed specifically with this goal in mind. And neither of them supports updating multiple sources in parallel. So they are slow when there are lots of inputs.

Note: this is _not_ a replacement for `nix flakes`. `nix flakes` is for managing external nix expression (aka "flakes") dependencies, and because of that it provides functionalities this tool cannot provide. For example, flake input following is not possible with this tool.
