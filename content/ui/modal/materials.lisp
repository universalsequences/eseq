;; eseq.modal.materials — materials for eseq.modal objects, as plain data.
;;
;; A material is a dict; an object is made of one, or offers a menu of
;; several:
;;
;;   (import eseq.modal :as m)
;;   (import eseq.modal.materials :as mat)
;;   (m/string 'str :made-of mat/nylon)
;;   (m/plate 'body :made-of (list mat/spruce mat/maple mat/aluminium))
;;
;; Your own materials are more data: (mat/material :name "gut" ...) or a
;; tweaked copy, (merge mat/steel :name "spring steel" :young 2.1e11).
;;
;; Values are typical handbook figures (density kg/m^3, Young's modulus Pa,
;; Poisson's ratio); woods are along the grain. The first nine follow the
;; Modalys material table that the original keyword materials came from.

(module eseq.modal.materials)

(export material material?
        uranium steel brass aluminium granite nylon carbon diamond spruce
        copper bronze titanium silver gold lead glass ice maple rosewood)

;; A material.
;;   name     what the panel's material menu shows
;;   density  kg/m^3
;;   young    Young's modulus, Pa; with density it sets how stiff the object
;;            is for its mass, which stretches a string's upper partials
;;            and sets a plate's pitch
;;   poisson  Poisson's ratio; shapes a plate's pitch (strings ignore it)
(def material (&key (name "material") (density 1000) (young 1e9) (poisson 0.3))
  (dict :kind :material :name name :density density :young young :poisson poisson))

;; True when x is a material.
;;   x  anything
(def material? (x) (if (map? x) (= (get x :kind) :material) false))

(def uranium   (material :name "uranium"   :density 19100 :young 2.08e11 :poisson 0.23))
(def steel     (material :name "steel"     :density 7850  :young 2.0e11  :poisson 0.29))
(def brass     (material :name "brass"     :density 8500  :young 1.0e11  :poisson 0.34))
(def aluminium (material :name "aluminium" :density 2700  :young 7.0e10  :poisson 0.33))
(def granite   (material :name "granite"   :density 2700  :young 4.0e10  :poisson 0.25))
(def nylon     (material :name "nylon"     :density 1140  :young 3.0e9   :poisson 0.4))
(def carbon    (material :name "carbon"    :density 1400  :young 1.1e12  :poisson 0.3))
(def diamond   (material :name "diamond"   :density 3500  :young 1.22e12 :poisson 0.2))
(def spruce    (material :name "spruce"    :density 450   :young 1.1e10  :poisson 0.37))
(def copper    (material :name "copper"    :density 8960  :young 1.17e11 :poisson 0.34))
;; bell bronze (about 80% copper, 20% tin)
(def bronze    (material :name "bronze"    :density 8600  :young 1.05e11 :poisson 0.34))
(def titanium  (material :name "titanium"  :density 4500  :young 1.16e11 :poisson 0.32))
(def silver    (material :name "silver"    :density 10490 :young 8.3e10  :poisson 0.37))
(def gold      (material :name "gold"      :density 19300 :young 7.9e10  :poisson 0.42))
(def lead      (material :name "lead"      :density 11340 :young 1.6e10  :poisson 0.44))
;; soda-lime glass
(def glass     (material :name "glass"     :density 2500  :young 7.0e10  :poisson 0.22))
(def ice       (material :name "ice"       :density 917   :young 9.0e9   :poisson 0.33))
(def maple     (material :name "maple"     :density 650   :young 1.2e10  :poisson 0.35))
(def rosewood  (material :name "rosewood"  :density 850   :young 1.6e10  :poisson 0.35))
