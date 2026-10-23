// Package quietconfig keeps rclone from logging "Couldn't find configuration directory" when
// the library loads. rclone's fs/config picks its config file in an init function, before the
// host can pass HOME through RcloneSetenv (Android starts apps without one), and logs errors
// when it finds no home. The host then uses no config file at all (`config/setpath ""`), so
// that choice never matters.
//
// RCLONE_CONFIG set (even empty) tells fs/config a config was given, which skips the search's
// errors. Go initialises the ready package that sorts first by import path, so this package
// (".../rclone/filebridge/quietconfig", which needs only os) runs before ".../rclone/fs/config".
// The main package's init, which runs after every package it imports, takes the variable back.
package quietconfig

import "os"

// Set is whether this package set RCLONE_CONFIG (the host had not), so main can unset it.
var Set bool

func init() {
	if _, given := os.LookupEnv("RCLONE_CONFIG"); !given {
		Set = os.Setenv("RCLONE_CONFIG", "") == nil
	}
}
