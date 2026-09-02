// Plain numbers rather than BigInt: every value the loop holds stays under 2^53, so a
// double represents each one exactly and the arithmetic is not approximating anything.
// BigInt would be the wrong comparison -- it is not what anybody writes for arithmetic
// this size, and it would be measuring an allocator.
let sum = 0;
for (let i = 1; i <= ITERATIONS; i++) {
  sum = (sum + i) % 1000000007;
}
console.log(sum);
