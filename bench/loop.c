// sum = (sum + i) mod 1000000007, a dependent chain: each iteration needs the last one's
// answer, so nothing here vectorises and what is measured is one add and one remainder.
#include <stdio.h>
#include <stdint.h>
int main(void) {
    uint64_t sum = 0;
    for (uint64_t i = 1; i <= ITERATIONS_ULL; i++) sum = (sum + i) % 1000000007ULL;
    printf("%llu\n", (unsigned long long)sum);
}
