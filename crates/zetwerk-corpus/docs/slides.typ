#import "@preview/touying:0.8.0": *
#import themes.simple: *

#show: simple-theme.with(aspect-ratio: "16-9")

= Iterated Averages

== The scheme

Replace each point by a weighted mean of its neighbours:

$ y_i = lambda x_i + (1 - lambda) x_(i+1) $

#pause

Repeat until one point is left.

== Main result

- The diameter shrinks by a factor $rho = max(lambda, 1 - lambda)$ per step.
- The constant is sharp.
- Nothing depends on the dimension.

== Sketch of proof

+ Write differences as telescoping sums.
+ Use the shared endpoint of consecutive averages.
+ Sum a geometric series.

#pause

*Conclusion:* geometric convergence.
