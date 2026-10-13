#!/usr/bin/perl
# Generates the Unicode simple case mapping tables in kelpc/std/str.kp.
#
# Source: the Unicode Character Database as shipped with Perl's Unicode::UCD
# (perl 5.42.3, Unicode 16.0.0). The simple mappings are the UnicodeData.txt
# fields 12 (uppercase) and 13 (lowercase), read through charinfo()'s
# `upper` and `lower` keys. Only code points that change under case mapping
# are visited: the Changes_When_Uppercased and Changes_When_Lowercased
# properties name every such code point.
#
# Usage: perl kelpc/tools/gen_case_table.pl kelpc/std/str.kp
#
# The file must hold the two marker lines below. The generator rewrites only
# the lines between them, so running it twice changes nothing.
use strict;
use warnings;
use Unicode::UCD qw(charinfo prop_invlist);

my $DBEGIN = '// BEGIN GENERATED: Unicode simple case table data (kelpc/tools/gen_case_table.pl)';
my $DEND = '// END GENERATED: Unicode simple case table data';
my $BEGIN = '// BEGIN GENERATED: Unicode simple case lookup (kelpc/tools/gen_case_table.pl)';
my $END = '// END GENERATED: Unicode simple case lookup';

my $target = shift @ARGV or die "usage: $0 kelpc/std/str.kp\n";
my $ver = Unicode::UCD::UnicodeVersion();

my %cand;
for my $prop ('Changes_When_Uppercased', 'Changes_When_Lowercased') {
    my @inv = prop_invlist($prop);
    for (my $i = 0; $i < @inv; $i += 2) {
        my $start = $inv[$i];
        my $end = ($i + 1 < @inv) ? $inv[$i + 1] - 1 : 0x10FFFF;
        $cand{$_} = 1 for $start .. $end;
    }
}

my (@up, @lo);
for my $cp (sort { $a <=> $b } keys %cand) {
    my $ci = charinfo($cp);
    my $u = $ci->{upper};
    my $l = $ci->{lower};
    push @up, [$cp, hex($u)] if $u ne '' && hex($u) != $cp;
    push @lo, [$cp, hex($l)] if $l ne '' && hex($l) != $cp;
}

# Kelp wants `const` lines before any `fun`, so the table data is its own
# marked block, which the file keeps after its `impl` lines.
my @data;
push @data, "$DBEGIN";
push @data, "// Unicode $ver simple case mappings, one entry per code point that changes.";
push @data, "// Generated: do not edit by hand; rerun the generator instead.";
push @data, "// Each table is one string of 16 lowercase hex digits per entry: the code";
push @data, "// point (8 digits), then its simple mapping (8 digits), sorted by code point.";
push @data, "";
push @data, sprintf("const CASE_UPPER_DATA: str = \"%s\"", join('', map { sprintf('%08x%08x', $_->[0], $_->[1]) } @up));
push @data, sprintf("const CASE_LOWER_DATA: str = \"%s\"", join('', map { sprintf('%08x%08x', $_->[0], $_->[1]) } @lo));
push @data, "$DEND";

my @out;
push @out, "$BEGIN";
push @out, "// Lookup over the table data above. A code point with no entry maps to itself.";
push @out, "";
push @out, "/// The eight hex digits at byte `at` of `data`, read as a u32.";
push @out, "fun case_hex32(data: &str, at: usize) => u32 {";
push @out, "    var acc: u32 = 0";
push @out, "    var k: usize = 0";
push @out, "    while k < 8 {";
push @out, "        val c = byte(data, at + k).to_u32()";
push @out, "        var d: u32 = 0";
push @out, "        if c >= 97 { d = c - 87 } else { d = c - 48 }";
push @out, "        acc = acc * 16 + d";
push @out, "        k = k + 1";
push @out, "    }";
push @out, "    return acc";
push @out, "}";
push @out, "";
push @out, "/// The mapping of `cp` in `data` (a table built as above), or `cp` itself.";
push @out, "fun case_lookup(data: &str, cp: u32) => u32 {";
push @out, "    var lo: usize = 0";
push @out, "    var hi: usize = len(data) / 16";
push @out, "    while lo < hi {";
push @out, "        val mid = lo + (hi - lo) / 2";
push @out, "        val key = case_hex32(data, mid * 16)";
push @out, "        if key == cp { return case_hex32(data, mid * 16 + 8) }";
push @out, "        if key < cp { lo = mid + 1 } else { hi = mid }";
push @out, "    }";
push @out, "    return cp";
push @out, "}";
push @out, "";
push @out, "/// The simple uppercase mapping of code point `cp`, or `cp` itself.";
push @out, "fun case_upper_cp(cp: u32) => u32 = case_lookup(CASE_UPPER_DATA, cp)";
push @out, "";
push @out, "/// The simple lowercase mapping of code point `cp`, or `cp` itself.";
push @out, "fun case_lower_cp(cp: u32) => u32 = case_lookup(CASE_LOWER_DATA, cp)";
push @out, "$END";

open my $fh, '<', $target or die "cannot read $target: $!\n";
my @lines = <$fh>;
close $fh;
chomp @lines;
# Replace each marked region (data, then lookup) with its generated lines.
my %region = ($DBEGIN => [$DEND, \@data], $BEGIN => [$END, \@out]);
my @new;
my $seen = 0;
for (my $i = 0; $i < @lines; $i++) {
    if (exists $region{$lines[$i]}) {
        my ($stop, $body) = @{ $region{$lines[$i]} };
        my $j = $i + 1;
        $j++ while $j < @lines && $lines[$j] ne $stop;
        die "$target: no '$stop' after line " . ($i + 1) . "\n" if $j >= @lines;
        push @new, @$body;
        $i = $j;
        $seen++;
    } else {
        push @new, $lines[$i];
    }
}
die "$target: missing one of the generated marker pairs\n" unless $seen == 2;
open my $wh, '>', $target or die "cannot write $target: $!\n";
print $wh map { "$_\n" } @new;
close $wh;
printf "wrote %d upper and %d lower entries (Unicode %s) into %s\n", scalar @up, scalar @lo, $ver, $target;
