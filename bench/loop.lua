local sum = 0
for i = 1, ITERATIONS do sum = (sum + i) % 1000000007 end
print(sum)
