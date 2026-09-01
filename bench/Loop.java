public class Loop {
    public static void main(String[] args) {
        long sum = 0;
        for (long i = 1; i <= ITERATIONS_L; i++) sum = (sum + i) % 1000000007L;
        System.out.println(sum);
    }
}
