# jpass: build, test and install (per user, no root needed).
#
#   make              build the release binary
#   make test         run the tests
#   make install      build, install for the current user, (re)start the background instance
#   make uninstall    remove everything `install` created (never touches pass.rage)
#
# `jpass install` does the actual per-user setup (it is also what runs on
# the very first launch), so the layout is defined in one place: src/install.rs.

CARGO ?= cargo
BIN   := target/release/jpass
UNAME := $(shell uname -s)

.DEFAULT_GOAL := build
.PHONY: build test run install uninstall start stop restart clean help

build:
	$(CARGO) build --release --locked

test:
	$(CARGO) test --release --locked

# Run from the build directory (shows the window; second launch toggles it).
run: build
	$(BIN)

install: build stop
	$(BIN) install
	@$(MAKE) --no-print-directory start

restart: stop start

stop:
	-@pkill -x jpass 2>/dev/null; true

ifeq ($(UNAME),Darwin)
start:
	open -g -a jpass --args hidden
else
start:
	@( setsid nohup "$(HOME)/.local/bin/jpass" hidden >/dev/null 2>&1 & )
endif

# Removes the installed app, launcher, autostart entry and first-run marker.
# Your encrypted vault (pass.rage and pass.rage.bak in your pCloud folder) is NOT touched.
uninstall: stop
ifeq ($(UNAME),Darwin)
	-launchctl bootout gui/$$(id -u)/com.jpass.app 2>/dev/null
	rm -f "$(HOME)/Library/LaunchAgents/com.jpass.app.plist"
	rm -rf /Applications/jpass.app "$(HOME)/Applications/jpass.app"
	rm -rf "$(HOME)/Library/Application Support/jpass"
else
	rm -f "$(HOME)/.local/bin/jpass"
	rm -f "$(HOME)/.local/share/applications/jpass.desktop"
	rm -f "$(HOME)/.config/autostart/jpass.desktop"
	rm -rf "$(HOME)/.config/jpass"
endif

clean:
	$(CARGO) clean

help:
	@sed -n '1,9p' Makefile
