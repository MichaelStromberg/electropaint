/*
 * drand48_fixture.c -- print IRIX drand48() output for fixed seeds, as
 * reference data for the Rust clone. Usage: drand48_fixture SEED N
 * Each line: index, value with 17 significant digits, raw IEEE bits.
 */
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv)
{
	long seed, n, i;
	union { double d; unsigned long w[2]; } u;

	if (argc != 3) { fprintf(stderr, "usage: %s seed n\n", argv[0]); return 2; }
	seed = atol(argv[1]);
	n = atol(argv[2]);
	srand48(seed);
	for (i = 0; i < n; i++) {
		u.d = drand48();
		printf("%ld %.17g %08lx%08lx\n", i, u.d, u.w[0], u.w[1]);
	}
	return 0;
}
