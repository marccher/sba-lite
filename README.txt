SBA Lite - Quick Start Guide
============================

SBA Lite runs as a zero-dependency setup requiring only this executable and a configuration file.

1. CONFIGURATION
Create a file named 'instances.toml' in the same directory as this executable.
You can find an example configuration in the 'instances.example.toml' file included in this package.

2. RUNNING THE SERVER
* Linux:
  chmod +x sbalite
  ./sbalite
* macOS (Apple Silicon):
  xattr -d com.apple.quarantine sbalite
  chmod +x sbalite
  ./sbalite
* Windows:
  .\sbalite.exe

The HTTP server will start. Open http://localhost:9001 in your browser.

FULL DOCUMENTATION & BENCHMARKS
For the complete guide, architecture details, diagrams, and Docker setup,
please visit the official repository: https://github.com
