<?php
$CONFIG = array (
  'htaccess.RewriteBase' => '/',
  'memcache.local' => '\\OC\\Memcache\\APCu',
  'apps_paths' => 
  array (
    0 => 
    array (
      'path' => '/var/www/nextcloud/apps',
      'url' => '/apps',
      'writable' => false,
    ),
  ),
  'instanceid' => 'ocfixture0001',
  'passwordsalt' => 'FIXTURE-NOT-A-REAL-SALT',
  'secret' => 'FIXTURE-NOT-A-REAL-SECRET',
  'trusted_domains' => 
  array (
    0 => 'cloud.example.lan',
  ),
  // 'datadirectory' => '/var/www/nextcloud/data',
  'datadirectory' => '/mnt/ncdata',
  'dbtype' => 'mysql',
  'version' => '29.0.4.1',
  'overwrite.cli.url' => 'https://cloud.example.lan',
  'dbname' => 'nextcloud',
  'dbhost' => 'localhost',
  'dbuser' => 'nextcloud',
  'dbpassword' => 'FIXTURE-NOT-A-REAL-PASSWORD',
  'installed' => true,
  'maintenance' => false,
);
